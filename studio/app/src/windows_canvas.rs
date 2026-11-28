use super::Studio;
use gpui::{div, img, prelude::*, AnyElement, ObjectFit, RenderImage, Window};
use psf_studio_render::{Captured, CapturedPixels};
use std::sync::Arc;

pub(super) fn image(frame: Captured) -> Option<Arc<RenderImage>> {
    let CapturedPixels::Bgra8(pixels) = frame.pixels else {
        return None;
    };
    let image = image::RgbaImage::from_raw(frame.width, frame.height, pixels)?;
    // GPUI interprets these bytes as BGRA for its sprite atlas.
    Some(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(image)
    ])))
}

impl Studio {
    pub(super) fn windows_canvas(&mut self, window: &mut Window) -> Option<AnyElement> {
        if let Some(error) = self.canvas.read(|shared| shared.readout.error.clone()) {
            return Some(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .child(format!("The canvas could not render. {error}"))
                    .into_any_element(),
            );
        }
        if let Some(image) = self.canvas.read_mut(|shared| shared.next_image.take())
            && let Some(old) = self.canvas_image.replace(image)
        {
            let _ = window.drop_image(old);
        }
        self.canvas_image.as_ref().map(|image| {
            img(image.clone())
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .object_fit(ObjectFit::Fill)
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_image_keeps_bgra_channels_and_pixel_dimensions() {
        let pixels = vec![0, 32, 255, 255, 255, 32, 0, 255];
        let image = image(Captured {
            width: 2,
            height: 1,
            pixels: CapturedPixels::Bgra8(pixels.clone()),
        })
        .expect("BGRA frame");
        assert_eq!(image.as_bytes(0), Some(pixels.as_slice()));
        assert_eq!(image.size(0), gpui::size(2.into(), 1.into()));
    }
}
