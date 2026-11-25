//! A PSF-Studio document's sources: PSF Guard's channel stacks.
//!
//! PSF Guard stacks each channel. Studio starts from those stacks and does
//! the color processing itself. This module lists a target's channel stacks
//! on a server, downloads them, and registers them onto one grid with
//! Seiza, which is the first thing every later step needs.

use crate::color_steps::{
    correct_channel_backgrounds, place_on_reference, register_channels, BackgroundEvent,
};
use crate::image_steps::{Domain, ImageShape, Region};
use crate::server::stack_preview::color::{
    StackBackgroundExtraction, StackColorRole, StackColorSource,
};
use crate::studio::{background_corrected_channel, RegionReader, StudioError};
use crate::studio_flow::role_name;
use anyhow::{anyhow, bail, Context, Result};
use seiza_background::BackgroundFit;
use seiza_stacking::{GpuOptions, LinearImage};
use serde::de::DeserializeOwned;
use std::path::Path;

/// One database on a PSF Guard server.
pub struct Server {
    client: reqwest::Client,
    /// `http://host:port/api/db/<database>`.
    database_url: String,
    token: Option<String>,
}

impl Server {
    pub fn new(origin: &str, database: &str, token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            database_url: format!("{}/api/db/{database}", origin.trim_end_matches('/')),
            token,
        }
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        let request = self.client.get(url);
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Reads the `data` of the server's reply to a GET.
    async fn data<T: DeserializeOwned>(&self, url: String) -> Result<T> {
        let response = self.get(url).send().await?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .with_context(|| format!("reading the server's reply ({status})"))?;
        if !status.is_success() {
            let reason = body["error"].as_str().unwrap_or("no reason given");
            bail!("the server refused the request ({status}): {reason}");
        }
        serde_json::from_value(body["data"].clone()).context("reading the server's reply")
    }

    /// A target's current channel stacks, one per channel, in the server's
    /// order. A channel with several current stacks is refused: the caller
    /// must say which stack it means.
    pub async fn channel_stacks(
        &self,
        project_id: i32,
        target_id: i32,
    ) -> Result<Vec<StackColorSource>> {
        let catalog: serde_json::Value = self
            .data(format!(
                "{}/projects/{project_id}/stack-previews/color",
                self.database_url
            ))
            .await?;
        let target = catalog["targets"]
            .as_array()
            .and_then(|targets| {
                targets
                    .iter()
                    .find(|target| target["target_id"] == target_id)
            })
            .ok_or_else(|| {
                anyhow!("project {project_id} has no channel stacks for target {target_id}")
            })?;
        let stacks: Vec<StackColorSource> =
            serde_json::from_value(target["source_candidates"].clone())
                .context("reading the target's channel stacks")?;
        let mut one_each: Vec<StackColorSource> = Vec::new();
        for stack in stacks {
            if one_each.iter().any(|kept| kept.role == stack.role) {
                bail!(
                    "target {target_id} has more than one current {} stack; choosing between them is not built yet",
                    stack.filter_name
                );
            }
            one_each.push(stack);
        }
        Ok(one_each)
    }

    /// Downloads a channel stack's FITS to `path`, a piece at a time.
    pub async fn download_stack(&self, stack: &StackColorSource, path: &Path) -> Result<u64> {
        use std::io::Write;
        let url = format!(
            "{}/stack-previews/{}/{}/fits?v={}",
            self.database_url, stack.job_id, stack.group_index, stack.artifact_revision
        );
        let mut response = self
            .get(url)
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("downloading the {} stack", stack.filter_name))?;
        let partial = path.with_extension("fits.part");
        let mut file = std::fs::File::create(&partial)
            .with_context(|| format!("creating {}", partial.display()))?;
        let mut bytes = 0;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk)?;
            bytes += chunk.len() as u64;
        }
        file.sync_all()?;
        std::fs::rename(&partial, path).with_context(|| format!("writing {}", path.display()))?;
        Ok(bytes)
    }
}

/// How one channel was placed on the reference channel's grid.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelPlacement {
    pub role: StackColorRole,
    /// `None` for the reference channel, which is not moved.
    pub registration: Option<ChannelRegistration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelRegistration {
    pub rms_error_pixels: f64,
    pub drift_pixels: f64,
    pub matched_stars: usize,
}

/// Channel stacks on one grid: a linear image with one channel per plane.
///
/// Without background correction there is one plane per stack, in the order
/// the stacks were given. With it there are two per stack: first every
/// channel uncorrected, then every channel corrected in full. A strength
/// between is a blend of the two, because Seiza's correction changes each
/// sample in proportion to the strength and resampling keeps proportions.
/// So a person can move the strength and see the result at once, with no
/// refit.
///
/// The channels stay as separate planes, as Seiza gave them. A region is
/// interleaved when it is read, so the channels are never copied whole.
pub struct RegisteredChannels {
    shape: ImageShape,
    planes: Vec<Vec<f32>>,
    /// A name for each plane: the channel's role, and for a corrected
    /// plane [`background_corrected_channel`] of it.
    pub names: Vec<String>,
    pub placements: Vec<ChannelPlacement>,
    /// Each channel's fitted background, when the channels were corrected.
    pub backgrounds: Vec<(StackColorRole, BackgroundFit)>,
}

impl RegisteredChannels {
    /// Reads each channel stack, corrects its background when `background`
    /// is given, and places it on the first stack's grid.
    ///
    /// The steps and their order are the ones PSF Guard's color previews
    /// run: the code in `color_steps`. Registration is measured on the
    /// stacks as they came. Each background is fitted and corrected in the
    /// channel's own frame, before resampling, so the empty border a
    /// resample leaves is never fitted.
    ///
    /// Every stack is held in memory while this runs, twice when the
    /// background is corrected. A mosaic too large for that needs these
    /// steps by tile.
    pub fn prepare(
        stacks: &[(StackColorRole, &Path)],
        background: Option<&StackBackgroundExtraction>,
        gpu: &GpuOptions,
    ) -> Result<Self> {
        if stacks.is_empty() {
            bail!("a document needs at least one channel stack");
        }
        let images = stacks
            .iter()
            .map(|(_, path)| open_mono(path))
            .collect::<Result<Vec<_>>>()?;
        let roles: Vec<StackColorRole> = stacks.iter().map(|(role, _)| *role).collect();
        let reference = (roles[0], images[0].width, images[0].height);

        let others: Vec<(StackColorRole, &LinearImage)> =
            roles.iter().copied().zip(&images).skip(1).collect();
        let registrations = register_channels((roles[0], &images[0]), &others, gpu, |_, _| Ok(()))
            .map_err(anyhow::Error::msg)?;
        let mut placements = vec![ChannelPlacement {
            role: roles[0],
            registration: None,
        }];
        placements.extend(
            registrations
                .iter()
                .map(|(role, registration)| ChannelPlacement {
                    role: *role,
                    registration: Some(ChannelRegistration {
                        rms_error_pixels: registration.rms_error_pixels,
                        drift_pixels: registration.drift_pixels,
                        matched_stars: registration.matched_stars,
                    }),
                }),
        );

        // Places every channel of `images` on the reference grid.
        let place = |images: Vec<LinearImage>| -> Result<Vec<Vec<f32>>> {
            let mut images = images.into_iter();
            let mut planes = vec![images.next().expect("checked above").data];
            for (image, (role, registration)) in images.zip(&registrations) {
                let placed = place_on_reference(*role, &image, reference, registration, gpu)
                    .map_err(anyhow::Error::msg)?;
                planes.push(placed.data);
            }
            Ok(planes)
        };

        let mut names = roles
            .iter()
            .map(|role| role_name(*role))
            .collect::<Result<Vec<_>, _>>()?;
        let mut backgrounds = Vec::new();
        let planes = match background {
            None => place(images)?,
            Some(extraction) => {
                // Corrected in full: the strength is applied when the two
                // planes are blended.
                let full = StackBackgroundExtraction {
                    strength: 1.0,
                    ..extraction.clone()
                };
                let mut corrected = images.clone();
                let mut channels: Vec<(StackColorRole, &mut LinearImage)> =
                    roles.iter().copied().zip(corrected.iter_mut()).collect();
                // Catalog protection needs a plate solve of the stack, which
                // Studio does not read yet, so no region is protected.
                correct_channel_backgrounds(
                    &mut channels,
                    &full,
                    |_| Vec::new(),
                    |role, event| {
                        if let BackgroundEvent::Corrected { fit, .. } = event {
                            backgrounds.push((role, fit.clone()));
                        }
                        Ok(())
                    },
                )
                .map_err(anyhow::Error::msg)?;
                let corrected_names: Vec<String> = names
                    .iter()
                    .map(|name| background_corrected_channel(name))
                    .collect();
                names.extend(corrected_names);
                let mut planes = place(images)?;
                planes.extend(place(corrected)?);
                planes
            }
        };
        Ok(Self {
            shape: ImageShape {
                width: reference.1,
                height: reference.2,
                channels: planes.len(),
            },
            planes,
            names,
            placements,
            backgrounds,
        })
    }
}

fn open_mono(path: &Path) -> Result<LinearImage> {
    let frame = crate::image_io::open_linear_frame(path)
        .with_context(|| format!("reading {}", path.display()))?;
    if frame.image.channels != 1 {
        bail!(
            "{} has {} channels; a channel stack has one",
            path.display(),
            frame.image.channels
        );
    }
    Ok(frame.image)
}

impl RegionReader for RegisteredChannels {
    fn shape(&self) -> ImageShape {
        self.shape
    }

    fn domain(&self) -> Domain {
        Domain::Linear
    }

    fn peak_bytes(&self, region: Region) -> u64 {
        (region.width as u64)
            .saturating_mul(region.height as u64)
            .saturating_mul(self.shape.channels as u64)
            .saturating_mul(size_of::<f32>() as u64)
    }

    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        region.check(self.shape)?;
        let channels = self.shape.channels;
        let mut samples = vec![0.0; region.width * region.height * channels];
        for (channel, plane) in self.planes.iter().enumerate() {
            for row in 0..region.height {
                let start = (region.y + row) * self.shape.width + region.x;
                let source = &plane[start..start + region.width];
                let out = &mut samples[row * region.width * channels..];
                for (column, &value) in source.iter().enumerate() {
                    out[column * channels + channel] = value;
                }
            }
        }
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels(width: usize, height: usize, planes: Vec<Vec<f32>>) -> RegisteredChannels {
        RegisteredChannels {
            shape: ImageShape {
                width,
                height,
                channels: planes.len(),
            },
            planes,
            names: Vec::new(),
            placements: Vec::new(),
            backgrounds: Vec::new(),
        }
    }

    #[test]
    fn a_region_is_the_channels_interleaved() {
        // Two 3 x 2 channels: one counts up, one counts up from 100.
        let first: Vec<f32> = (0..6).map(|v| v as f32).collect();
        let second: Vec<f32> = (0..6).map(|v| 100.0 + v as f32).collect();
        let image = channels(3, 2, vec![first, second]);
        let whole = image.read(image.shape().region()).unwrap();
        assert_eq!(
            whole,
            [0.0, 100.0, 1.0, 101.0, 2.0, 102.0, 3.0, 103.0, 4.0, 104.0, 5.0, 105.0]
        );
        let corner = image
            .read(Region {
                x: 1,
                y: 1,
                width: 2,
                height: 1,
            })
            .unwrap();
        assert_eq!(corner, [4.0, 104.0, 5.0, 105.0]);
    }

    #[test]
    fn a_region_outside_the_image_is_refused() {
        let image = channels(3, 2, vec![vec![0.0; 6]]);
        let outside = Region {
            x: 2,
            y: 0,
            width: 2,
            height: 1,
        };
        assert!(image.read(outside).is_err());
    }
}
