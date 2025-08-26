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

        let mut frm = Frame::new(x, y, 196, 56, "");
        let state = Rc::from(RefCell::from(LaunchButtonState::Updating));
        frm.draw({
            let state = state.clone();
            move |f| {
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
            move |f, ev| match ev {
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
