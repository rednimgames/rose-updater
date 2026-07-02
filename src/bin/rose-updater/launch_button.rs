use fltk::frame::*;
use fltk::image::*;
use fltk::{enums::*, prelude::*};
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

#[derive(Clone, Copy)]
pub enum LaunchButtonState {
    Update,
    Updating,
    Play,
}

/// Brighten the RGB channels of an image's pixels while preserving its alpha
/// channel, so a hover highlight respects the button's rounded/transparent
/// edges. Returns the pixel data along with its dimensions and color depth.
fn brightened_image(png_bytes: &[u8], factor: f32) -> (Vec<u8>, i32, i32, ColorDepth) {
    let img = PngImage::from_data(png_bytes).unwrap();
    let (w, h, depth) = (img.data_w(), img.data_h(), img.depth());
    let channels = depth as usize;
    let has_alpha = channels == 4;

    let mut data = img.to_rgb_data();
    for (i, byte) in data.iter_mut().enumerate() {
        // Skip the alpha byte (every 4th) when the image has an alpha channel.
        if has_alpha && i % channels == channels - 1 {
            continue;
        }
        *byte = ((*byte as f32) * factor).min(255.0) as u8;
    }

    (data, w, h, depth)
}

pub struct LaunchButton {
    frm: Frame,
    state: Rc<RefCell<LaunchButtonState>>,
}

impl LaunchButton {
    pub fn new(x: i32, y: i32) -> Self {
        let update_state =
            include_bytes!("../../../res/Launcher_Alpha_UpdateButton.png") as &[u8];
        let updating_state =
            include_bytes!("../../../res/Launcher_Alpha_UpdatingButton.png") as &[u8];
        let play_state = include_bytes!("../../../res/Launcher_Alpha_PlayButton.png") as &[u8];

        // Pre-compute a brightened variant of the play button for the hover
        // effect so we don't rebuild it on every redraw.
        let (play_hover_data, hover_w, hover_h, hover_depth) = brightened_image(play_state, 1.25);

        let mut frm = Frame::new(x, y, 196, 56, "");
        let state = Rc::from(RefCell::from(LaunchButtonState::Updating));
        let hovered = Rc::from(RefCell::from(false));
        frm.draw({
            let state = state.clone();
            let hovered = hovered.clone();
            move |f| {
                // Draw the brightened image when hovering the play button.
                if matches!(*state.borrow(), LaunchButtonState::Play) && *hovered.borrow() {
                    let mut hover_img =
                        RgbImage::new(&play_hover_data, hover_w, hover_h, hover_depth).unwrap();
                    hover_img.draw(f.x(), f.y(), hover_w, hover_h);
                    return;
                }

                let image_data = match *state.borrow() {
                    LaunchButtonState::Update => update_state,
                    LaunchButtonState::Updating => updating_state,
                    LaunchButtonState::Play => play_state,
                };
                let mut png = PngImage::from_data(image_data).unwrap();
                png.draw(f.x(), f.y(), png.width(), png.height());
            }
        });
        frm.handle({
            let state = state.clone();
            let hovered = hovered.clone();
            move |f, ev| match ev {
                Event::Enter => {
                    // Only the play button is clickable, so only it gets the
                    // pointer cursor and hover highlight.
                    if matches!(*state.borrow(), LaunchButtonState::Play) {
                        if let Some(mut win) = f.window() {
                            win.set_cursor(Cursor::Hand);
                        }
                        *hovered.borrow_mut() = true;
                        f.redraw();
                    }
                    true
                }
                Event::Leave => {
                    if let Some(mut win) = f.window() {
                        win.set_cursor(Cursor::Default);
                    }
                    if *hovered.borrow() {
                        *hovered.borrow_mut() = false;
                        f.redraw();
                    }
                    true
                }
                Event::Released => {
                    let prev = *state.borrow();
                    match prev {
                        LaunchButtonState::Update => {}
                        LaunchButtonState::Updating => {
                            *state.borrow_mut() = LaunchButtonState::Play;
                        }
                        LaunchButtonState::Play => {}
                    }
                    f.do_callback();
                    f.redraw();
                    true
                }
                _ => false,
            }
        });
        Self { frm, state }
    }

    pub fn change_state(&mut self, state: LaunchButtonState) {
        *self.state.borrow_mut() = state;
    }
}

impl Deref for LaunchButton {
    type Target = Frame;

    fn deref(&self) -> &Self::Target {
        &self.frm
    }
}

impl DerefMut for LaunchButton {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frm
    }
}
