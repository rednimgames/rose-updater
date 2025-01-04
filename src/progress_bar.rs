use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::Arc;

use fltk::enums::{Align, Color, Font, FrameType};
use fltk::frame::*;
use fltk::image::*;
use fltk::{draw, prelude::*};
use humansize::{format_size, DECIMAL};

pub struct ProgressBar {
    bar: Frame,
    min: Arc<AtomicUsize>,
    max: Arc<AtomicUsize>,
    value: Arc<AtomicUsize>,
    _max_size: Arc<AtomicI32>,
    is_zero: Arc<AtomicBool>,
}

impl ProgressBar {
    pub fn new(x: i32, y: i32) -> Self {
        let progress_bar_bytes = include_bytes!("../res/Launcher_Alpha_LoadingBar.png");
        let font_bytes = include_bytes!("../res/JosefinSans-Bold.ttf");
        let black_bytes = include_bytes!("../res/ariblk.ttf");
        #[allow(invalid_from_utf8_unchecked)]
        unsafe {
            Font::set_font(Font::Helvetica, std::str::from_utf8_unchecked(font_bytes));
            Font::set_font(Font::Courier, std::str::from_utf8_unchecked(black_bytes));
        }
        let mut bar = Frame::new(x, y, 546, 56 + 30, "");
        let min = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let value = Arc::new(AtomicUsize::new(0));
        let max_size = Arc::new(AtomicI32::new(0));
        let is_zero = Arc::new(AtomicBool::new(false));
        bar.draw({
            let min = min.clone();
            let max = max.clone();
            let value = value.clone();
            let max_size = max_size.clone();
            let is_zero = is_zero.clone();
            move |b| {
                let mut png = PngImage::from_data(progress_bar_bytes).unwrap();

                let value = value.load(Ordering::Relaxed);
                let max = max.load(Ordering::Relaxed);
                let min = min.load(Ordering::Relaxed);
                let is_zero = is_zero.load(Ordering::Relaxed);

                let value = if value > max { max } else { value };

                let width = if max - min == 0 {
                    0
                } else {
                    (value * png.width() as usize) / (max - min)
                };

                let width = if is_zero { png.width() as usize } else { width };

                png.draw(b.x(), b.y(), width as i32, png.height());

                draw::set_font(Font::Courier, 18);
                draw::set_draw_color(Color::White);
                // right side %
                let percentage = if max - min == 0 {
                    0
                } else {
                    (value * 100) / (max - min)
                };
                let percentage = if is_zero { 100 } else { percentage };
                let percentage = format!("{}%", percentage);
                let size = draw::width(&percentage);
                if size + 20.0 <= width as f64 || is_zero {
                    draw::draw_text2(
                        &percentage,
                        b.x(),
                        b.y(),
                        (width - 10) as i32,
                        png.height(),
                        Align::Right,
                    );
                } else if max == 0 {
                    draw::draw_text2(
                        "Downloading patch metadata",
                        b.x(),
                        b.y(),
                        b.width(),
                        png.height(),
                        Align::Center,
                    );
                }

                // underneath total size
                let data_size = format!(
                    "{} / {}",
                    format_size(value, DECIMAL),
                    format_size(max, DECIMAL)
                );
                let data_size = if is_zero {
                    "- B / - B".to_string()
                } else {
                    data_size
                };

                draw::set_font(Font::Helvetica, 12);
                let mut size = draw::width(&data_size) as i32;
                if size > max_size.load(Ordering::Relaxed) {
                    max_size.store(size, Ordering::Relaxed);
                } else {
                    size = max_size.load(Ordering::Relaxed);
                }
                draw::draw_box(
                    FrameType::FlatBox,
                    b.x() + b.width() - size,
                    b.y() + b.height() - 25,
                    size,
                    30,
                    Color::from_rgb(33, 26, 39),
                );
                draw::set_font(Font::Helvetica, 12);
                draw::set_draw_color(Color::White);
                draw::draw_text2(
                    &data_size,
                    b.x(),
                    b.y() + b.height() - 30,
                    b.width(),
                    30,
                    Align::Right,
                );
            }
        });
        Self {
            bar,
            min,
            max,
            value,
            _max_size: max_size,
            is_zero,
        }
    }

    pub fn set_minimum(&mut self, value: usize) {
        self.min.store(value, Ordering::Relaxed);
    }

    pub fn set_maximum(&mut self, value: usize) {
        self.max.store(value, Ordering::Relaxed);
        if value == 0 {
            self.is_zero.store(true, Ordering::Relaxed);
        }
    }

    pub fn set_value(&mut self, value: usize) {
        self.value.store(value, Ordering::Relaxed);
    }

    pub fn minimum(&self) -> usize {
        self.min.load(Ordering::Relaxed)
    }

    pub fn maximum(&self) -> usize {
        self.max.load(Ordering::Relaxed)
    }

    pub fn value(&self) -> usize {
        self.value.load(Ordering::Relaxed)
    }
}

impl Deref for ProgressBar {
    type Target = Frame;

    fn deref(&self) -> &Self::Target {
        &self.bar
    }
}

impl DerefMut for ProgressBar {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.bar
    }
}
