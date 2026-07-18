use std::path::PathBuf;

use seiza::objects::{
    GeometryData, GeometryQuality, GeometryRole, ObjectCatalog, ObjectCatalogData, ObjectContour,
    ObjectDetails, ObjectGeometry, ObjectKind, ObjectMetadata, SkyObject,
};

fn main() {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: generate_astrometry_e2e_catalog <output>");
    let object = SkyObject {
        kind: ObjectKind::OpenCluster,
        ra: 130.107013851174,
        dec: 19.6601508517091,
        mag: Some(3.1),
        major_arcmin: Some(95.0),
        minor_arcmin: Some(95.0),
        position_angle_deg: None,
        name: "M 44".to_string(),
        common_name: "Beehive Cluster".to_string(),
        metadata: ObjectMetadata {
            id: "openngc:NGC2632".to_string(),
            source: "OpenNGC".to_string(),
            aliases: vec!["NGC 2632".to_string(), "Praesepe".to_string()],
            ..Default::default()
        },
    };
    let mut details = ObjectDetails::from_canonical(&object);
    let ra_radius = 0.12 / object.dec.to_radians().cos();
    details.geometries.push(ObjectGeometry {
        id: "openngc:NGC2632#e2e-outline".to_string(),
        source_record_id: "openngc:NGC2632".to_string(),
        role: GeometryRole::PreferredRender,
        quality: GeometryQuality::Curated,
        method: "PSF Guard deterministic e2e fixture".to_string(),
        evidence: "Synthetic contour around the catalog center".to_string(),
        data: GeometryData::OutlineSet {
            level: Some("fixture-boundary".to_string()),
            contours: vec![ObjectContour {
                closed: true,
                vertices: vec![
                    (object.ra - ra_radius, object.dec),
                    (object.ra, object.dec + 0.08),
                    (object.ra + ra_radius, object.dec),
                    (object.ra, object.dec - 0.08),
                ],
            }],
        },
    });

    ObjectCatalog::from_data(ObjectCatalogData {
        objects: vec![object],
        details: vec![details],
        provenance: Default::default(),
    })
    .expect("valid fixture catalog")
    .write_to(&output)
    .expect("write fixture catalog");
}
