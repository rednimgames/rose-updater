use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use fltk::enums::{Align, Color, Font, FrameType};
use fltk::frame::*;
use fltk::image::*;
use fltk::{draw, prelude::*};
use humansize::{format_size, DECIMAL};

use crate::ProgressStage;

pub struct ProgressBar {
    bar: Frame,
    min: Arc<AtomicUsize>,
    max: Arc<AtomicUsize>,
    value: Arc<AtomicUsize>,
    _max_size: Arc<AtomicI32>,
    stage: Arc<AtomicU64>,
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
        let stage = Arc::new(AtomicU64::new(ProgressStage::None as u64));

        bar.draw({
            let min = min.clone();
            let max = max.clone();
            let value = value.clone();
            let max_size = max_size.clone();
            let stage = stage.clone();

            move |b| {
                let mut png = PngImage::from_data(progress_bar_bytes).unwrap();

                let value = value.load(Ordering::Relaxed);
                let max = max.load(Ordering::Relaxed);
                let min = min.load(Ordering::Relaxed);
                let is_zero = max == 0;
                let stage = ProgressStage::from(stage.load(Ordering::Relaxed) as usize);

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

                let message = match stage {
                    ProgressStage::FetchingMetadata => "Fetching metadata".into(),
                    ProgressStage::UpdatingUpdater => {
                        format!(
                            "Updating updater - {} / {}",
                            format_size(value, DECIMAL),
                            format_size(max, DECIMAL)
                        )
                    }
                    ProgressStage::CheckingFiles => {
                        format!("Checking local files - {} / {}", value, max)
                    }
                    ProgressStage::DownloadingUpdates => {
                        format!(
                            "Downloading Updates - {} / {}",
                            format_size(value, DECIMAL),
                            format_size(max, DECIMAL)
                        )
                    }
                    _ => "".into(),
                };

                draw::set_font(Font::Helvetica, 12);
                let mut size = draw::width(&message) as i32;
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
                    &message,
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
            stage,
        }
    }

    pub fn set_minimum(&mut self, value: usize) {
        self.min.store(value, Ordering::Relaxed);
    }

    pub fn set_maximum(&mut self, value: usize) {
        self.max.store(value, Ordering::Relaxed);
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

    pub fn current_stage(&self) -> ProgressStage {
        ProgressStage::from(self.stage.load(Ordering::Relaxed) as usize)
    }

    pub fn set_stage(&mut self, value: ProgressStage) {
        self.stage.store(value as u64, Ordering::Relaxed);
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
