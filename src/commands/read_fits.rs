use crate::directory_tree::DirectoryTree;
use anyhow::Result;
use fitrs::Fits;
use serde_json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn read_fits(path: &str, verbose: bool, format: &str) -> Result<()> {
    let path = Path::new(path);

    if path.is_file() {
        // Single file
        read_single_fits(path, verbose, format)?;
    } else if path.is_dir() {
        // Directory of files
        read_fits_directory(path, verbose, format)?;
    } else {
        return Err(anyhow::anyhow!(
            "Path does not exist or is not accessible: {}",
            path.display()
        ));
    }

    Ok(())
}

fn read_single_fits(path: &Path, verbose: bool, format: &str) -> Result<()> {
    let metadata = read_fits_metadata(path)?;

    match format.to_lowercase().as_str() {
        "json" => {
            let json_output = if verbose {
                serde_json::to_string_pretty(&metadata)?
            } else {
                // For non-verbose JSON, create a simpler structure
                let simplified = create_simplified_metadata(&metadata);
                serde_json::to_string_pretty(&simplified)?
            };
            println!("{}", json_output);
        }
        "csv" => {
            output_csv_single(&metadata, verbose)?;
        }
        _ => {
            println!("Reading FITS file: {}\n", path.display());
            let formatted = format_fits_metadata(&metadata, verbose);
            println!("{}", formatted);
        }
    }

    Ok(())
}

fn read_fits_directory(dir: &Path, verbose: bool, format: &str) -> Result<()> {
    // Build directory tree cache and get FITS files
    let directory_tree = DirectoryTree::build(dir)?;
    let fits_files: Vec<PathBuf> = directory_tree.get_fits_files().into_iter().cloned().collect();

    if fits_files.is_empty() {
        match format.to_lowercase().as_str() {
            "json" => println!("[]"),
            "csv" => {
                // Print CSV header even if no files
                println!("filename,width,height,bit_depth,date_obs,object,exposure,filter,telescope,instrument,gain,ccd_temp,binning,ra,dec,hfr,stars,fwhm");
            }
            _ => println!("No FITS files found in directory."),
        }
        return Ok(());
    }

    let mut successful_metadata = Vec::new();
    let mut error_count = 0;

    // Read all files and collect metadata
    for file_path in &fits_files {
        match read_fits_metadata(file_path) {
            Ok(metadata) => successful_metadata.push(metadata),
            Err(_) => error_count += 1,
        }
    }

    // Output based on format
    match format.to_lowercase().as_str() {
        "json" => {
            let json_output = if verbose {
                serde_json::to_string_pretty(&successful_metadata)?
            } else {
                let simplified: Vec<_> = successful_metadata
                    .iter()
                    .map(create_simplified_metadata)
                    .collect();
                serde_json::to_string_pretty(&simplified)?
            };
            println!("{}", json_output);
        }
        "csv" => {
            output_csv_directory(&successful_metadata, verbose)?;
        }
        _ => {
            println!("Scanning directory: {}\n", dir.display());
            println!("Found {} FITS files\n", fits_files.len());

            for (index, metadata) in successful_metadata.iter().enumerate() {
                println!("File {}/{}:", index + 1, fits_files.len());
                let formatted = format_fits_metadata(metadata, verbose);
                println!("{}", formatted);

                // Add separator between files if not last
                if index < successful_metadata.len() - 1 {
                    println!("{:-<60}", "");
                }
            }

            println!("\nSummary:");
            println!("  Successfully read: {}", successful_metadata.len());
            if error_count > 0 {
                println!("  Errors: {}", error_count);
            }
        }
    }

    Ok(())
}

/// Metadata extracted from a FITS file
#[derive(Debug, serde::Serialize)]
pub struct FitsMetadata {
    pub filename: String,
    pub headers: Vec<HeaderInfo>,
    pub primary_header: HashMap<String, String>,
    pub image_info: Option<ImageInfo>,
}

#[derive(Debug, serde::Serialize)]
pub struct HeaderInfo {
    pub hdu_index: usize,
    pub hdu_name: Option<String>,
    pub keywords: HashMap<String, String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ImageInfo {
    pub width: usize,
    pub height: usize,
    pub bit_depth: i32,
    pub dimensions: Vec<usize>,
}

/// Extract image info from a FITS data array
fn extract_image_info_from_array<T>(
    array: &fitrs::FitsDataArray<T>,
    hdu: &fitrs::Hdu,
) -> Option<ImageInfo> {
    let shape = &array.shape;
    if shape.len() >= 2 {
        let width = shape[0];
        let height = shape[1];

        // Try to get bit depth from header
        let bit_depth = hdu
            .value("BITPIX")
            .and_then(|v| {
                let s = format!("{:?}", v);
                // Extract number from debug string like "Integer(16)" or "CharacterString(\"16\")"
                if let Some(start) = s.find(|c: char| c.is_ascii_digit() || c == '-') {
                    let mut end = start;
                    while end < s.len() {
                        let ch = s.chars().nth(end).unwrap();
                        if ch.is_ascii_digit() || (end == start && ch == '-') {
                            end += 1;
                        } else {
                            break;
                        }
                    }
                    s[start..end].parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);

        Some(ImageInfo {
            width,
            height,
            bit_depth,
            dimensions: shape.clone(),
        })
    } else {
        None
    }
}

/// Read metadata from a FITS file using fitrs
pub fn read_fits_metadata(path: &Path) -> Result<FitsMetadata> {
    let fits = Fits::open(path)
        .map_err(|e| anyhow::anyhow!("Failed to open FITS file {}: {:?}", path.display(), e))?;

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Read primary HDU
    let hdu = fits
        .get(0)
        .ok_or_else(|| anyhow::anyhow!("No HDU found in FITS file"))?;

    // Extract headers as key-value pairs
    let mut primary_header = HashMap::new();

    // Common FITS header keywords to extract
    let keywords = vec![
        "SIMPLE",
        "BITPIX",
        "NAXIS",
        "NAXIS1",
        "NAXIS2",
        "EXTEND",
        "OBJECT",
        "DATE-OBS",
        "EXPTIME",
        "FILTER",
        "TELESCOP",
        "INSTRUME",
        "OBSERVER",
        "GAIN",
        "CCD-TEMP",
        "XBINNING",
        "YBINNING",
        "FOCALLEN",
        "FOCUSPOS",
        "OBJCTRA",
        "OBJCTDEC",
        "RA",
        "DEC",
        "AIRMASS",
        "SWCREATE",
        "HFR",
        "STARS",
        "STARSFWHM",
        "EXTNAME",
        "OBJNAME",
        "TARGET",
        "EXPOSURE",
        "FILTERNAME",
        "STARHFR",
        "MEANHFR",
        "STARCOUNT",
        "NSTARS",
        "FWHM",
        "MEANFWHM",
    ];

    // Try to read each keyword from the header
    for keyword in keywords {
        if let Some(value) = hdu.value(keyword) {
            // Convert HeaderValue to string using Debug formatting for now
            let value_str = format!("{:?}", value);
            primary_header.insert(keyword.to_string(), value_str);
        }
    }

    // Extract image info from the actual data
    let image_info = match hdu.read_data() {
        fitrs::FitsData::FloatingPoint32(array) => extract_image_info_from_array(&array, &hdu),
        fitrs::FitsData::FloatingPoint64(array) => extract_image_info_from_array(&array, &hdu),
        fitrs::FitsData::IntegersI32(array) => extract_image_info_from_array(&array, &hdu),
        fitrs::FitsData::IntegersU32(array) => extract_image_info_from_array(&array, &hdu),
        _ => None,
    };

    let headers = vec![HeaderInfo {
        hdu_index: 0,
        hdu_name: primary_header.get("EXTNAME").cloned(),
        keywords: primary_header.clone(),
    }];

    Ok(FitsMetadata {
        filename,
        headers,
        primary_header,
        image_info,
    })
}

/// Format FITS metadata for display
pub fn format_fits_metadata(metadata: &FitsMetadata, verbose: bool) -> String {
    let mut output = String::new();

    output.push_str(&format!("Filename: {}\n", metadata.filename));

    if let Some(ref img_info) = metadata.image_info {
        output.push_str(&format!(
            "Image: {}x{} ({}-bit)\n",
            img_info.width, img_info.height, img_info.bit_depth
        ));
    }

    // Display key headers
    let key_headers = vec![
        ("OBJECT", "Object"),
        ("DATE-OBS", "Observation Date"),
        ("EXPTIME", "Exposure"),
        ("FILTER", "Filter"),
        ("TELESCOP", "Telescope"),
        ("INSTRUME", "Instrument"),
        ("OBSERVER", "Observer"),
        ("GAIN", "Gain"),
        ("CCD-TEMP", "CCD Temperature"),
        ("XBINNING", "Binning"),
        ("FOCALLEN", "Focal Length"),
        ("FOCUSPOS", "Focus Position"),
        ("OBJCTRA", "RA"),
        ("OBJCTDEC", "DEC"),
        ("AIRMASS", "Airmass"),
    ];

    output.push_str("\nKey Headers:\n");
    for (key, label) in key_headers {
        if let Some(value) = metadata.primary_header.get(key) {
            output.push_str(&format!("  {}: {}\n", label, value));
        }
    }

    // N.I.N.A. specific headers
    if metadata.primary_header.contains_key("SWCREATE") {
        output.push_str("\nN.I.N.A. Headers:\n");
        if let Some(v) = metadata.primary_header.get("SWCREATE") {
            output.push_str(&format!("  Software: {}\n", v));
        }
        if let Some(v) = metadata.primary_header.get("HFR") {
            output.push_str(&format!("  HFR: {}\n", v));
        }
        if let Some(v) = metadata.primary_header.get("STARS") {
            output.push_str(&format!("  Stars: {}\n", v));
        }
        if let Some(v) = metadata.primary_header.get("STARSFWHM") {
            output.push_str(&format!("  FWHM: {}\n", v));
        }
    }

    if verbose {
        output.push_str("\nAll Headers:\n");
        let mut keys: Vec<_> = metadata.primary_header.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(value) = metadata.primary_header.get(key) {
                output.push_str(&format!("  {}: {}\n", key, value));
            }
        }
    }

    output
}



#[derive(serde::Serialize)]
struct SimplifiedFitsMetadata {
    filename: String,
    width: Option<usize>,
    height: Option<usize>,
    bit_depth: Option<i32>,
    date_obs: Option<String>,
    object: Option<String>,
    exposure: Option<String>,
    filter: Option<String>,
    telescope: Option<String>,
    instrument: Option<String>,
    gain: Option<String>,
    ccd_temp: Option<String>,
    binning: Option<String>,
    ra: Option<String>,
    dec: Option<String>,
    hfr: Option<String>,
    stars: Option<String>,
    fwhm: Option<String>,
}

fn create_simplified_metadata(metadata: &FitsMetadata) -> SimplifiedFitsMetadata {
    SimplifiedFitsMetadata {
        filename: metadata.filename.clone(),
        width: metadata.image_info.as_ref().map(|i| i.width),
        height: metadata.image_info.as_ref().map(|i| i.height),
        bit_depth: metadata.image_info.as_ref().map(|i| i.bit_depth),
        date_obs: metadata.primary_header.get("DATE-OBS").cloned(),
        object: metadata
            .primary_header
            .get("OBJECT")
            .or_else(|| metadata.primary_header.get("OBJNAME"))
            .or_else(|| metadata.primary_header.get("TARGET"))
            .cloned(),
        exposure: metadata
            .primary_header
            .get("EXPTIME")
            .or_else(|| metadata.primary_header.get("EXPOSURE"))
            .cloned(),
        filter: metadata
            .primary_header
            .get("FILTER")
            .or_else(|| metadata.primary_header.get("FILTERNAME"))
            .cloned(),
        telescope: metadata.primary_header.get("TELESCOP").cloned(),
        instrument: metadata.primary_header.get("INSTRUME").cloned(),
        gain: metadata.primary_header.get("GAIN").cloned(),
        ccd_temp: metadata.primary_header.get("CCD-TEMP").cloned(),
        binning: metadata.primary_header.get("XBINNING").cloned(),
        ra: metadata
            .primary_header
            .get("OBJCTRA")
            .or(metadata.primary_header.get("RA"))
            .cloned(),
        dec: metadata
            .primary_header
            .get("OBJCTDEC")
            .or(metadata.primary_header.get("DEC"))
            .cloned(),
        hfr: metadata
            .primary_header
            .get("HFR")
            .or_else(|| metadata.primary_header.get("STARHFR"))
            .or_else(|| metadata.primary_header.get("MEANHFR"))
            .cloned(),
        stars: metadata
            .primary_header
            .get("STARS")
            .or_else(|| metadata.primary_header.get("STARCOUNT"))
            .or_else(|| metadata.primary_header.get("NSTARS"))
            .cloned(),
        fwhm: metadata
            .primary_header
            .get("STARSFWHM")
            .or_else(|| metadata.primary_header.get("FWHM"))
            .or_else(|| metadata.primary_header.get("MEANFWHM"))
            .cloned(),
    }
}

fn output_csv_single(metadata: &FitsMetadata, verbose: bool) -> Result<()> {
    if verbose {
        // For verbose mode, output all headers as key-value pairs
        println!("filename,key,value");
        println!(
            "{},filename,{}",
            escape_csv(&metadata.filename),
            escape_csv(&metadata.filename)
        );

        for (key, value) in &metadata.primary_header {
            println!(
                "{},{},{}",
                escape_csv(&metadata.filename),
                escape_csv(key),
                escape_csv(value)
            );
        }
    } else {
        // Standard CSV format
        println!("filename,width,height,bit_depth,date_obs,object,exposure,filter,telescope,instrument,gain,ccd_temp,binning,ra,dec,hfr,stars,fwhm");
        let simplified = create_simplified_metadata(metadata);
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            escape_csv(&simplified.filename),
            simplified.width.map(|v| v.to_string()).unwrap_or_default(),
            simplified.height.map(|v| v.to_string()).unwrap_or_default(),
            simplified
                .bit_depth
                .map(|v| v.to_string())
                .unwrap_or_default(),
            escape_csv(&simplified.date_obs.unwrap_or_default()),
            escape_csv(&simplified.object.unwrap_or_default()),
            escape_csv(&simplified.exposure.unwrap_or_default()),
            escape_csv(&simplified.filter.unwrap_or_default()),
            escape_csv(&simplified.telescope.unwrap_or_default()),
            escape_csv(&simplified.instrument.unwrap_or_default()),
            escape_csv(&simplified.gain.unwrap_or_default()),
            escape_csv(&simplified.ccd_temp.unwrap_or_default()),
            escape_csv(&simplified.binning.unwrap_or_default()),
            escape_csv(&simplified.ra.unwrap_or_default()),
            escape_csv(&simplified.dec.unwrap_or_default()),
            escape_csv(&simplified.hfr.unwrap_or_default()),
            escape_csv(&simplified.stars.unwrap_or_default()),
            escape_csv(&simplified.fwhm.unwrap_or_default())
        );
    }
    Ok(())
}

fn output_csv_directory(metadata_list: &[FitsMetadata], verbose: bool) -> Result<()> {
    if verbose {
        // For verbose mode, output all headers as key-value pairs
        println!("filename,key,value");
        for metadata in metadata_list {
            println!(
                "{},filename,{}",
                escape_csv(&metadata.filename),
                escape_csv(&metadata.filename)
            );
            for (key, value) in &metadata.primary_header {
                println!(
                    "{},{},{}",
                    escape_csv(&metadata.filename),
                    escape_csv(key),
                    escape_csv(value)
                );
            }
        }
    } else {
        // Standard CSV format
        println!("filename,width,height,bit_depth,date_obs,object,exposure,filter,telescope,instrument,gain,ccd_temp,binning,ra,dec,hfr,stars,fwhm");
        for metadata in metadata_list {
            let simplified = create_simplified_metadata(metadata);
            println!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                escape_csv(&simplified.filename),
                simplified.width.map(|v| v.to_string()).unwrap_or_default(),
                simplified.height.map(|v| v.to_string()).unwrap_or_default(),
                simplified
                    .bit_depth
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                escape_csv(&simplified.date_obs.unwrap_or_default()),
                escape_csv(&simplified.object.unwrap_or_default()),
                escape_csv(&simplified.exposure.unwrap_or_default()),
                escape_csv(&simplified.filter.unwrap_or_default()),
                escape_csv(&simplified.telescope.unwrap_or_default()),
                escape_csv(&simplified.instrument.unwrap_or_default()),
                escape_csv(&simplified.gain.unwrap_or_default()),
                escape_csv(&simplified.ccd_temp.unwrap_or_default()),
                escape_csv(&simplified.binning.unwrap_or_default()),
                escape_csv(&simplified.ra.unwrap_or_default()),
                escape_csv(&simplified.dec.unwrap_or_default()),
                escape_csv(&simplified.hfr.unwrap_or_default()),
                escape_csv(&simplified.stars.unwrap_or_default()),
                escape_csv(&simplified.fwhm.unwrap_or_default())
            );
        }
    }
    Ok(())
}

fn escape_csv(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
