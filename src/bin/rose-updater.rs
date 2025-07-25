use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
    pin::pin,
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, RwLock,
    },
};

use anyhow::Context;
use clap::Parser;
use eframe::egui::{self, Widget};
use futures_util::{future::try_join_all, StreamExt};
use reqwest::Url;
use serde::Deserialize;
use tokio::{fs, runtime::Runtime};
use tracing_subscriber::{filter::LevelFilter, EnvFilter, FmtSubscriber};

use rose_update::{
    style::{FONT_POPPINS_BOLD, FONT_POPPINS_LIGHT, FONT_POPPINS_MEDIUM, FONT_POPPINS_REGULAR},
    widgets::NewsLabel,
    LocalManifest, LocalManifestFileEntry, RemoteFileDownloader, RemoteManifest,
};

const BACKGROUND_IMAGE: egui::ImageSource = egui::include_image!("../../res/bg.png");
const BUTTON_PLAY_IMAGE: egui::ImageSource = egui::include_image!("../../res/button-play.png");
const BUTTON_UPDATING_IMAGE: egui::ImageSource =
    egui::include_image!("../../res/button-updating.png");
const PROGRESSBAR_LOADING_IMAGE: egui::ImageSource =
    egui::include_image!("../../res/progressbar-loading.png");
const PROGRESSBAR_DONE_IMAGE: egui::ImageSource =
    egui::include_image!("../../res/progressbar-done.png");

const CLOSE_IMAGE: egui::ImageSource = egui::include_image!("../../res/close.svg");
const MINIMIZE_IMAGE: egui::ImageSource = egui::include_image!("../../res/minimize.svg");
const ROSEONLINE_IMAGE: egui::ImageSource = egui::include_image!("../../res/roseonline.png");
const REDNIMGAMES_IMAGE: egui::ImageSource = egui::include_image!("../../res/rednimgames.png");
const SETTINGS_IMAGE: egui::ImageSource = egui::include_image!("../../res/settings.svg");

const POPPINS_BOLD: &[u8] = include_bytes!("../../res/font/Poppins-Bold.ttf");
const POPPINS_LIGHT: &[u8] = include_bytes!("../../res/font/Poppins-Light.ttf");
const POPPINS_MEDIUM: &[u8] = include_bytes!("../../res/font/Poppins-Medium.ttf");
const POPPINS_REGULAR: &[u8] = include_bytes!("../../res/font/Poppins-Regular.ttf");

const ICON_BYTES: &[u8] = include_bytes!("../../res/client.png");

const LOCAL_MANIFEST_VERSION: usize = 1;
const TEXT_FILE_EXTENSIONS: &[&str; 1] = &["xml"];
const UPDATER_OLD_EXT: &str = "old";

#[derive(Clone, Parser, Debug)]
#[clap(about, version, author)]
struct Args {
    /// Remote archive URL
    #[clap(long, default_value = "https://updates2.roseonlinegame.com")]
    url: String,

    #[clap(long, default_value = "https://www.roseonlinegame.com/api/v1/news")]
    news_url: String,

    /// Output directory
    #[clap(long, default_value = ".")]
    output: PathBuf,

    /// Name of manifest file
    #[clap(long, default_value = "manifest.json")]
    manifest_name: String,

    /// Don't update the updater
    #[clap(long)]
    skip_updater: bool,

    /// Ignore the local manifest in the cache and force all files to be checked
    #[clap(long)]
    force_recheck: bool,

    /// Ignore the local manifest in the cache and force the updater to be rechecked for updates
    #[clap(long)]
    force_recheck_updater: bool,

    /// Enable/Disable debug logs
    #[clap(long)]
    debug: bool,

    /// Verify all local files
    #[clap(long)]
    verify: bool,

    /// Executable to run after updating
    #[clap(long, default_value = "trose.exe")]
    exe: PathBuf,

    /// Working directory to run the executable
    #[clap(long, default_value = ".")]
    exe_dir: PathBuf,

    /// When update is complete and we are able to launch the game, auto launch it
    #[clap(long)]
    auto_launch: bool,

    /// Arguments for the executable
    /// NOTE: This must be the last option in the command line to properly handle
    #[clap(
        multiple_values = true,
        default_value = "--init --server connect.roseonlinegame.com",
        value_delimiter = ' '
    )]
    exe_args: Vec<String>,
}

#[derive(Default)]
struct ProgressState {
    progress_amount: AtomicUsize,
    progress_total: AtomicUsize,
    progress_text: RwLock<String>,

    update_complete: AtomicBool,
    update_error: RwLock<Option<anyhow::Error>>,
}

#[derive(Default)]
pub enum NewsState {
    #[default]
    Fetching,
    Completed(News),
    Failed(anyhow::Error),
}

pub enum UpdaterError {
    UpdateError(anyhow::Error),
    CommandError(anyhow::Error),
    NewsError(anyhow::Error),
}

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct NewsCategory {
    id: u32,
    title: String,
    link: String,
}

#[allow(dead_code)]
#[derive(Deserialize)]
pub struct NewsItem {
    id: u32,
    title: String,
    short_description: String,
    description: String,
    image: String,
    category: NewsCategory,
    #[serde(with = "time::serde::rfc3339")]
    published_at: time::OffsetDateTime,
    link: String,
}

#[derive(Deserialize)]
pub struct News {
    data: Vec<NewsItem>,
}

#[derive(PartialEq)]
pub enum TitleBarAction {
    ToggleOptions,
}

#[derive(Default)]
pub enum ContentAreaView {
    #[default]
    News,
    Options,
    Error(UpdaterError),
}

#[derive(Default)]
struct UiState {
    progress_total: usize,
    progress_amount: usize,
    progress_text: String,

    enable_play_button: bool,
    launch_game: bool,
    content_view: ContentAreaView,
}

struct UpdaterApp {
    args: Args,
    runtime: Runtime,

    ui_state: UiState,
    progress_state: Arc<ProgressState>,
    news_state: Arc<RwLock<NewsState>>,
    update_process_handle: Option<tokio::task::JoinHandle<()>>,

    use_beta: bool,
}

impl UpdaterApp {
    pub fn new(args: Args) -> anyhow::Result<UpdaterApp> {
        let progress_state = Arc::new(ProgressState::default());
        let news_state = Arc::new(RwLock::new(NewsState::default()));
        let runtime = tokio::runtime::Runtime::new()?;
        let ui_state = UiState::default();

        let mut app = UpdaterApp {
            args,
            runtime,
            progress_state,
            news_state,
            ui_state,
            update_process_handle: None,
            use_beta: false,
        };

        app.run_news_process();
        app.run_update_process();

        Ok(app)
    }

    pub fn setup(&self, cc: &eframe::CreationContext<'_>) {
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            FONT_POPPINS_BOLD.to_owned(),
            egui::FontData::from_static(POPPINS_BOLD),
        );
        fonts.font_data.insert(
            FONT_POPPINS_LIGHT.to_owned(),
            egui::FontData::from_static(POPPINS_LIGHT),
        );
        fonts.font_data.insert(
            FONT_POPPINS_MEDIUM.to_owned(),
            egui::FontData::from_static(POPPINS_MEDIUM),
        );
        fonts.font_data.insert(
            FONT_POPPINS_REGULAR.to_owned(),
            egui::FontData::from_static(POPPINS_REGULAR),
        );

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, FONT_POPPINS_REGULAR.to_owned());

        fonts
            .families
            .entry(egui::FontFamily::Name(FONT_POPPINS_BOLD.into()))
            .or_default()
            .insert(0, FONT_POPPINS_BOLD.to_owned());

        fonts
            .families
            .entry(egui::FontFamily::Name(FONT_POPPINS_LIGHT.into()))
            .or_default()
            .insert(0, FONT_POPPINS_LIGHT.to_owned());

        fonts
            .families
            .entry(egui::FontFamily::Name(FONT_POPPINS_MEDIUM.into()))
            .or_default()
            .insert(0, FONT_POPPINS_MEDIUM.to_owned());

        cc.egui_ctx.set_fonts(fonts);
    }

    pub fn run_update_process(&mut self) {
        let update_args = self.args.clone();
        let app_progress_state = self.progress_state.clone();

        if let Some(handle) = self.update_process_handle.take() {
            handle.abort();
        }

        let handle = self.runtime.spawn(async move {
            if let Err(update_error) =
                update_process(&update_args, app_progress_state.clone()).await
            {
                let mut error = app_progress_state
                    .update_error
                    .write()
                    .expect("Update error poisoned");
                *error = Some(update_error);
            }
        });

        self.update_process_handle = Some(handle);
    }

    pub fn run_news_process(&self) {
        let news_state = self.news_state.clone();

        let news_url = self.args.news_url.clone();
        self.runtime.spawn(async move {
            match news_process(&news_url).await {
                Ok(news) => {
                    let mut news_state = news_state.write().expect("Failed to update news state");
                    *news_state = NewsState::Completed(news)
                }
                Err(e) => {
                    let mut news_state = news_state.write().expect("Failed to update news state");
                    *news_state = NewsState::Failed(e);
                }
            }
        });
    }

    pub fn launch_game(&self) -> anyhow::Result<()> {
        let exe_path = if self.use_beta {
            self.args.exe_dir.join("trose-new.exe")
        } else {
            self.args.exe_dir.join(&self.args.exe)
        };

        Command::new(exe_path)
            .current_dir(&self.args.exe_dir)
            .args(&exe_args)
            .spawn()?;

        Ok(())
    }
}

impl eframe::App for UpdaterApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Sync from threaded state to ui state
        self.ui_state.progress_amount = self.progress_state.progress_amount.load(Ordering::Relaxed);
        self.ui_state.progress_total = self.progress_state.progress_total.load(Ordering::Relaxed);

        if let Ok(progress_text) = self.progress_state.progress_text.try_read() {
            if progress_text.as_str() != self.ui_state.progress_text.as_str() {
                self.ui_state.progress_text = progress_text.clone();
            }
        }

        self.ui_state.enable_play_button =
            self.progress_state.update_complete.load(Ordering::Relaxed);

        if let Ok(mut lock) = self.progress_state.update_error.try_write() {
            let err = lock.take();
            if let Some(err) = err {
                self.ui_state.content_view = ContentAreaView::Error(UpdaterError::UpdateError(err));
            }
        }

        if let Ok(lock) = self.news_state.try_read() {
            if let NewsState::Failed(ref err) = *lock {
                self.ui_state.content_view = ContentAreaView::Error(UpdaterError::NewsError(
                    anyhow::anyhow!(err.to_string()),
                ));
            }
        }

        // Draw main UI
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Image::new(BACKGROUND_IMAGE).paint_at(ui, ctx.screen_rect());

            let logo_size = egui::vec2(100.0, 56.0);

            let top_left = egui::pos2(
                ctx.screen_rect().center_top().x - (logo_size.x / 2.0),
                ctx.screen_rect().center_top().y + 15.0,
            );
            let bottom_right = egui::pos2(top_left.x + logo_size.x, top_left.y + logo_size.y);

            let logo_rect = egui::Rect::from_two_pos(top_left, bottom_right);

            egui::Image::new(ROSEONLINE_IMAGE)
                .fit_to_exact_size(logo_size)
                .paint_at(ui, logo_rect);

            let title_bar_action = draw_title_bar(ui, frame);
            if title_bar_action.is_some_and(|a| a == TitleBarAction::ToggleOptions) {
                self.ui_state.content_view = match self.ui_state.content_view {
                    ContentAreaView::Options => ContentAreaView::News,
                    _ => ContentAreaView::Options,
                };
            }

            ui.spacing_mut().interact_size.y = 40.0;

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 20.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Image::new(REDNIMGAMES_IMAGE)
                                .fit_to_exact_size(egui::vec2(80.0, 20.0)),
                        );
                    },
                );

                draw_progress_area(ui, &mut self.ui_state);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                    ui.add_space(15.0);
                    ui.checkbox(&mut self.use_beta, "Use Beta Client")
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                });

                ui.with_layout(
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| match &self.ui_state.content_view {
                        ContentAreaView::News => {
                            if let Ok(lock) = self.news_state.try_read() {
                                draw_content_area(ui, &lock);
                            } else {
                                draw_content_area(ui, &NewsState::Fetching);
                            }
                        }
                        ContentAreaView::Options => {
                            ui.vertical_centered(|ui| {
                                ui.spacing_mut().button_padding.x = 10.0;

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Min),
                                    |ui| {
                                        ui.visuals_mut().widgets.inactive.weak_bg_fill =
                                            egui::Color32::TRANSPARENT;
                                        ui.visuals_mut().widgets.inactive.bg_stroke =
                                            egui::Stroke::NONE;
                                        ui.visuals_mut().widgets.active.weak_bg_fill =
                                            egui::Color32::TRANSPARENT;
                                        ui.visuals_mut().widgets.active.bg_stroke =
                                            egui::Stroke::NONE;
                                        ui.visuals_mut().widgets.hovered.weak_bg_fill =
                                            egui::Color32::RED;
                                        ui.visuals_mut().widgets.hovered.bg_stroke =
                                            egui::Stroke::NONE;
                                        ui.visuals_mut().widgets.hovered.weak_bg_fill =
                                            egui::Color32::from_black_alpha(50);

                                        if ui
                                            .add(egui::ImageButton::new(
                                                egui::Image::new(CLOSE_IMAGE)
                                                    .fit_to_exact_size(egui::vec2(16.0, 16.0)),
                                            ))
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            self.ui_state.content_view = ContentAreaView::News;
                                        }
                                    },
                                );

                                ui.add_space(20.0);

                                if ui
                                    .add(
                                        egui::Button::new("📁 Open Config Folder")
                                            .min_size(egui::vec2(300.0, 0.0)),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    if let Some(dir) = directories::ProjectDirs::from(
                                        "",
                                        "Rednim Games",
                                        "ROSE Online",
                                    ) {
                                        let _ = open::that(
                                            PathBuf::from("file://").join(dir.config_dir()),
                                        );
                                    }
                                }

                                if ui
                                    .add(
                                        egui::Button::new("🔨 Verify files")
                                            .min_size(egui::vec2(300.0, 0.0)),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    self.args.force_recheck = true;
                                    self.args.force_recheck_updater = true;
                                    self.run_update_process();
                                }

                                if ui
                                    .add(
                                        egui::Button::new("🌎 Support")
                                            .min_size(egui::vec2(300.0, 0.0)),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    let _ = open::that("https://support.roseonlinegame.com");
                                }
                            });
                        }
                        ContentAreaView::Error(error) => draw_error(ui, error),
                    },
                );
            });
        });

        if self.ui_state.launch_game || (self.ui_state.enable_play_button && self.args.auto_launch) {
            if let Err(e) = self.launch_game() {
                self.ui_state.content_view = ContentAreaView::Error(UpdaterError::CommandError(e));
            } else {
                frame.close();
            }
        }

        ctx.request_repaint();
    }
}

fn draw_error(ui: &mut egui::Ui, error: &UpdaterError) {
    let error_message = match error {
        UpdaterError::CommandError(e) => {
            format!(
                "There was an error launching the game client\n\nDetails: {}",
                e
            )
        }
        UpdaterError::UpdateError(e) => {
            format!("There was an error updating \n\nDetails: {}", e)
        }
        UpdaterError::NewsError(e) => {
            format!("There was an error fetching news items\n\nDetails: {}", e)
        }
    };

    ui.vertical_centered(|ui| {
        ui.add_space(150.0);
        ui.label(egui::RichText::new(&error_message).color(egui::Color32::WHITE));
        if ui
            .link("Copy Error")
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            ui.output_mut(|o| o.copied_text = error_message);
        };
    });
}

fn draw_title_bar(ui: &mut egui::Ui, app_frame: &mut eframe::Frame) -> Option<TitleBarAction> {
    let mut title_bar_action = None;

    let app_rect = ui.max_rect();

    let title_bar_height = 32.0;
    let title_bar_rect = {
        let mut rect = app_rect;
        rect.max.y = rect.min.y + title_bar_height;
        rect
    };

    let title_bar_response = ui.interact(
        title_bar_rect,
        egui::Id::new("title_bar"),
        egui::Sense::click(),
    );

    if title_bar_response.double_clicked() {
        app_frame.set_maximized(!app_frame.info().window_info.maximized);
    } else if title_bar_response.is_pointer_button_down_on() {
        app_frame.drag_window();
    }

    ui.allocate_ui_at_rect(title_bar_rect, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);

            ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::NONE;
            ui.visuals_mut().widgets.active.weak_bg_fill = egui::Color32::TRANSPARENT;
            ui.visuals_mut().widgets.active.bg_stroke = egui::Stroke::NONE;
            ui.visuals_mut().widgets.hovered.weak_bg_fill = egui::Color32::RED;
            ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;

            if ui
                .add(egui::ImageButton::new(
                    egui::Image::new(CLOSE_IMAGE).fit_to_exact_size(egui::vec2(16.0, 16.0)),
                ))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app_frame.close();
            }

            ui.visuals_mut().widgets.hovered.weak_bg_fill = egui::Color32::from_black_alpha(50);

            if ui
                .add(egui::ImageButton::new(
                    egui::Image::new(MINIMIZE_IMAGE).fit_to_exact_size(egui::vec2(16.0, 16.0)),
                ))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app_frame.set_minimized(true);
            }

            if ui
                .add(egui::ImageButton::new(
                    egui::Image::new(SETTINGS_IMAGE).fit_to_exact_size(egui::vec2(16.0, 16.0)),
                ))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                title_bar_action = Some(TitleBarAction::ToggleOptions);
            }
        });
    });

    title_bar_action
}

fn draw_content_area(ui: &mut egui::Ui, news: &NewsState) {
    ui.add_space(50.0);

    egui::Frame::none()
        .rounding(egui::Rounding::from(4.0))
        .inner_margin(egui::Margin {
            left: 15.0,
            right: 0.0,
            top: 0.0,
            bottom: 0.0,
        })
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Latest News")
                        .size(36.0)
                        .color(egui::Color32::WHITE)
                        .family(egui::FontFamily::Name("poppins-bold".into())),
                );

                let date_format =
                    time::format_description::parse("[year]-[month]-[day]").unwrap_or_default();

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| match news {
                        NewsState::Completed(news) => {
                            ui.spacing_mut().interact_size.y = 30.0;
                            for news_item in &news.data {
                                ui.horizontal(|ui| {
                                    let (date_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(100.0, 14.0),
                                        egui::Sense::hover(),
                                    );

                                    ui.allocate_ui_at_rect(date_rect, |ui| {
                                        ui.expand_to_include_x(date_rect.width());

                                        let date_str = news_item
                                            .published_at
                                            .format(&date_format)
                                            .unwrap_or("-".into());

                                        let date_text = egui::RichText::new(date_str)
                                            .color(egui::Color32::WHITE);
                                        ui.label(date_text);
                                    });

                                    ui.add(match news_item.category.id {
                                        1 => NewsLabel::News,
                                        2 => NewsLabel::Maintenance,
                                        3 => NewsLabel::Development,
                                        _ => NewsLabel::Custom(&news_item.category.title),
                                    });

                                    let link_text =
                                        egui::RichText::from(&news_item.title).size(14.0);

                                    ui.visuals_mut().hyperlink_color =
                                        egui::Color32::from_white_alpha(255);

                                    ui.add_space(10.0);
                                    if ui.link(link_text).clicked() {
                                        let _ = open::that(&news_item.link);
                                    };
                                });
                            }

                            ui.add_space(10.0);
                            ui.vertical_centered(|ui| {
                                if ui.link("Read More").clicked() {
                                    let _ = open::that("https://www.roseonlinegame.com/news/");
                                };
                            });
                        }
                        NewsState::Fetching | NewsState::Failed(_) => {
                            ui.spinner();
                        }
                    })
            });
        });
}

fn draw_progress_area(ui: &mut egui::Ui, state: &mut UiState) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
        let play_button_image_src = if state.enable_play_button {
            BUTTON_PLAY_IMAGE
        } else {
            BUTTON_UPDATING_IMAGE
        };

        let play_button_image =
            egui::Image::new(play_button_image_src).fit_to_exact_size(egui::vec2(140.0, 40.0));

        let play_button = egui::widgets::ImageButton::new(play_button_image)
            .frame(false)
            .ui(ui);

        if play_button.clicked() {
            state.launch_game = true;
        }

        if play_button.hovered() {
            if state.enable_play_button {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            } else {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Wait);
            }
        }

        draw_progress_bar(ui, state);
    });
}

fn draw_progress_bar(ui: &mut egui::Ui, state: &UiState) {
    let width = ui.available_size_before_wrap().x;
    let height = 40.0;

    let rounding = egui::Rounding::from(4.0);
    let (bg, _response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());

    // Draw progressbar background
    ui.painter().rect(
        bg,
        rounding,
        egui::Color32::from_black_alpha(150),
        egui::Stroke::new(0.5, egui::Color32::from_white_alpha(25)),
    );

    // Draw textured foreground
    let mut progress_percentage =
        (state.progress_amount as f32 / state.progress_total as f32).clamp(0.0, 1.0);

    if progress_percentage.is_nan() {
        progress_percentage = 0.0;
    }

    let fg_image_source = if progress_percentage == 1.0 {
        PROGRESSBAR_DONE_IMAGE
    } else {
        PROGRESSBAR_LOADING_IMAGE
    };

    let mut fg_rect = bg;
    fg_rect.set_width(fg_rect.width() * progress_percentage);

    egui::Image::new(fg_image_source).paint_at(ui, fg_rect);

    let text_with_percentage = if progress_percentage < 1.0 {
        format!(
            "{} ({:0.2}%)",
            &state.progress_text,
            progress_percentage * 100.0
        )
    } else {
        state.progress_text.clone()
    };

    // Draw text on progress bar
    let text = egui::WidgetText::from(&text_with_percentage);
    let galley = text.into_galley(ui, Some(false), f32::INFINITY, egui::TextStyle::Button);
    let text_pos = bg.center() - egui::vec2(0.0, galley.size().y / 2.0);
    let text_color = egui::Color32::WHITE;

    galley.paint_with_fallback_color(&ui.painter().with_clip_rect(bg), text_pos, text_color);
}

async fn news_process(news_url: &String) -> anyhow::Result<News> {
    Ok(reqwest::get(news_url).await?.json::<News>().await?)
}

async fn update_process(args: &Args, progress_state: Arc<ProgressState>) -> anyhow::Result<()> {
    let add_progress_amount = |amount: usize| {
        progress_state
            .progress_amount
            .fetch_add(amount, Ordering::SeqCst)
    };

    let set_progress_amount = |amount: usize| {
        progress_state
            .progress_amount
            .store(amount, Ordering::SeqCst);
    };

    let set_progress_total = |total: usize| {
        progress_state.progress_total.store(total, Ordering::SeqCst);
    };

    let set_progress_text = |text: &str| {
        if let Ok(mut progress_text) = progress_state.progress_text.write() {
            *progress_text = text.into();
        }
    };

    tracing::info!("Starting update process");
    set_progress_text("Starting update process");

    // Download the remote manifest file. The remote manifest is compared to the
    // local manifest to determine what needs to be updated.

    let remote_url =
        Url::parse(&args.url).context(format!("Failed to parse the url {}", args.url))?;
    let remote_manifest_url = remote_url.join(&args.manifest_name)?;

    tracing::info!(url = remote_url.as_str(), "Downloading remote manifest");
    set_progress_text("Downloading patch metadata");

    let remote_manifest = reqwest::get(remote_manifest_url)
        .await?
        .json::<RemoteManifest>()
        .await?;

    // The updater can use different "profiles" to use the same updater for
    // different clients or different download locations so the local manifest
    // needs to be found in the correct location.
    let local_manifest_path = args
        .output
        .join("updater")
        .join(remote_url.host_str().unwrap_or("default"))
        .join("local_manifest.json");

    // Read the local manifest file if we can. Otherwise we default to an empty
    // local manifest which we save as a new manifest later.
    tracing::info!(
        path =? local_manifest_path.display(),
        "Loading local manifest"
    );
    set_progress_text("Loading local patch data");

    let local_manifest = if let Ok(file) = File::open(&local_manifest_path) {
        match serde_json::from_reader(BufReader::new(file)) {
            Ok(manifest) => manifest,
            Err(_) => LocalManifest::default(),
        }
    } else {
        LocalManifest::default()
    };

    // Check if the updater itself needs an update by comparing remote updater
    // hash to local updater hash in manifest. If the updater does need an
    // update then this process will only download the updater and exit. A new
    // process will be started using the new updater to continue the download of
    // the rest of the files.
    let updater_output_path = args.output.join(&remote_manifest.updater.source_path);
    let updater_needs_update = remote_manifest.updater.source_hash != local_manifest.updater.hash;

    // When the updater is updated, the old updater is renamed since a running
    // process cannot be deleted. The old updater should be cleaned up when possible.
    let updater_output_path_old = updater_output_path.with_extension(UPDATER_OLD_EXT);
    let _ = fs::remove_file(&updater_output_path_old).await;

    // Update the updater if it needs to be updated.
    if !args.skip_updater && (args.force_recheck_updater || updater_needs_update) {
        tracing::info!("Updating updater");
        set_progress_text("Updating updater");

        // We cannot delete or modify a currently executing binary so we rename
        // the currently executing updater to allow us to download the new one
        // with the same name.
        tracing::info!("{}", updater_output_path.display());
        if updater_output_path.exists() {
            fs::rename(&updater_output_path, &updater_output_path_old)
                .await
                .context(format!(
                    "Failed to rename the updater from {} to {}",
                    updater_output_path.display(),
                    updater_output_path_old.display(),
                ))?;
        }

        let updater_url = remote_url.join(&remote_manifest.updater.path)?;
        let mut downloader = RemoteFileDownloader::new(
            &updater_url,
            &updater_output_path,
            reqwest::Client::builder().build()?,
        )
            .await?;

        let total_local_chunks_size = downloader.output_original_size();
        let total_download_chunk_count = downloader.chunk_download_count();

        // Verify local updater chunks. If there is no existing file then there
        // will be no verification step.
        {
            let mut local_chunks = pin!(downloader.load_output_chunks().await.peekable());
            if local_chunks.as_mut().peek().await.is_some() {
                set_progress_text("Verifying updater");
                set_progress_amount(0);
                set_progress_total(total_local_chunks_size);

                while let Some(chunk_size) = local_chunks.next().await {
                    let chunk_size = chunk_size?;
                    add_progress_amount(chunk_size);
                }

                set_progress_text("Updater verified");
                set_progress_amount(1);
                set_progress_total(1);
            };
        }

        // Clone different chunks, if the local file is already up to date then
        // nothing will be cloned.
        let mut updater_cloned = false;
        {
            let mut remote_chunks = pin!(downloader.clone_remote_chunks().await.peekable());
            if remote_chunks.as_mut().peek().await.is_some() {
                set_progress_text("Downloading updater");
                set_progress_amount(0);
                set_progress_total(total_download_chunk_count);

                while let Some(chunk_size) = remote_chunks.next().await {
                    let _chunk_size = chunk_size?;
                    add_progress_amount(1);
                }

                set_progress_text("Updater downloaded");
                set_progress_amount(1);
                set_progress_total(1);

                updater_cloned = true;
            }
        }

        if updater_cloned {
            // If the updater was cloned then the local manifest needs to be updated
            // since the new updater has been downloaded. Only the updater field
            // section in the manifest needs to be updated.
            let new_local_manifest = LocalManifest {
                version: LOCAL_MANIFEST_VERSION,
                updater: LocalManifestFileEntry {
                    path: remote_manifest.updater.source_path.clone(),
                    hash: remote_manifest.updater.source_hash.clone(),
                    size: remote_manifest.updater.source_size,
                },
                ..local_manifest
            };

            save_local_manifest(&new_local_manifest, &local_manifest_path).await?;

            // If the updater was updated then the update process should
            // continue with the new updater and this one needs to be
            // closed.
            //
            // All arguments passed to the original updater are forwarded to
            // the new one except ones that would cause issues (e.g.
            // force-recheck triggering an infinite loop).
            Command::new(&updater_output_path)
                .args(
                    env::args()
                        .skip(1)
                        .filter(|arg| !arg.contains("force-recheck-updater")),
                )
                .spawn()?;

            std::process::exit(0);
        }

        set_progress_text("Updater up-to-date");
    }

    // Check which files need to be updated by comparing data in the local
    // manifest to the remote manifest.
    let files_to_update = {
        tracing::debug!("Checking file to update");

        set_progress_text("Checking if files need updates");
        set_progress_amount(0);
        set_progress_total(remote_manifest.files.len());

        // Create a lookup table for our local cache data so we can compare to
        // remote manifest. We only want to update files that have changed to save
        // some time.
        let mut local_manifest_data: HashMap<PathBuf, LocalManifestFileEntry> = HashMap::new();
        for entry in &local_manifest.files {
            local_manifest_data.insert(PathBuf::from(&entry.path), entry.clone());
        }

        let mut files_to_update = Vec::new();
        for remote_entry in &remote_manifest.files {
            add_progress_amount(1);

            let output_path = args.output.join(&remote_entry.source_path);

            let needs_update = || {
                // If the force-recheck flag is set then all files will always be
                // checked for updates regardless of the values in the local
                // manifest.
                if args.force_recheck {
                    return true;
                }

                // If the local file does not already exist then it always needs to
                // be downloaded.
                if !output_path.exists() {
                    return true;
                }

                // Otherwise, only files that have a different hash in the remote
                // manifest should be updated.
                if let Some(local_entry) =
                    local_manifest_data.get(&PathBuf::from(&remote_entry.source_path))
                {
                    if local_entry.hash == remote_entry.source_hash {
                        return false;
                    }
                }

                true
            };

            if !needs_update() {
                continue;
            }

            let clone_url = remote_url.join(&remote_entry.path)?;
            files_to_update.push((clone_url, output_path));
        }

        set_progress_text("File checks completed");
        set_progress_amount(1);
        set_progress_total(1);

        files_to_update
    };

    // Setup remote file downloaders for the files that need data. Each of these
    // downloaders makes a network request to the remote archive to download
    // chunk meta data so they need to be executed concurrently later for better
    // performance.
    let downloaders = {
        tracing::debug!("Building downloaders");

        let http_client = reqwest::Client::builder().build()?;

        let mut downloaders = Vec::with_capacity(files_to_update.len());
        for (file_url, file_path) in &files_to_update {
            // Bitar doesn't handle text files well so when one of the text files
            // has changed, it is deleted and the full file is downloaded from the
            // remote archive.
            if let Some(ext) = file_path.extension().and_then(|s| s.to_str()) {
                if TEXT_FILE_EXTENSIONS.contains(&ext) {
                    std::fs::remove_file(file_path).ok();
                }
            }

            let http_client = http_client.clone();
            let file_path = file_path.clone();
            let file_url = file_url.clone();
            let progress_state = progress_state.clone();

            let downloader_task = tokio::spawn(async move {
                let downloader =
                    RemoteFileDownloader::new(&file_url, &file_path, http_client.clone()).await;

                progress_state
                    .progress_amount
                    .fetch_add(1, Ordering::SeqCst);

                downloader
            });
            downloaders.push(downloader_task);
        }

        // Create all the downloaders
        set_progress_text("Downloading update metadata");
        set_progress_amount(0);
        set_progress_total(downloaders.len());

        let downloaders: Vec<RemoteFileDownloader> = futures::future::try_join_all(downloaders)
            .await?
            .into_iter()
            .flatten()
            .collect();

        set_progress_text("Finished downloading update metadata");
        set_progress_amount(1);
        set_progress_total(1);

        downloaders
    };

    let mut verify_size = 0;
    let mut need_download_chunk_count = 0;

    // Verify local file chunks. If there is no existing file then there
    // will be no verifications
    tracing::info!("Verifying local chunks");

    for downloader in &downloaders {
        verify_size += downloader.output_original_size();
    }

    let mut verify_tasks: Vec<tokio::task::JoinHandle<anyhow::Result<RemoteFileDownloader>>> =
        Vec::new();

    for mut downloader in downloaders {
        tracing::debug!(path =? downloader.output_path().display(), size =? downloader.output_original_size(), "Loading local chunks");

        let progress_state = progress_state.clone();
        verify_tasks.push(tokio::spawn(async move {
            {
                let mut local_chunks = pin!(downloader.load_output_chunks().await);
                while let Some(chunk_size) = local_chunks.next().await {
                    let chunk_size = chunk_size?;

                    progress_state
                        .progress_amount
                        .fetch_add(chunk_size, Ordering::SeqCst);
                }
            }
            Ok(downloader)
        }));
    }

    set_progress_text("Verifying files");
    set_progress_amount(0);
    set_progress_total(verify_size);

    let downloaders: Vec<RemoteFileDownloader> = try_join_all(verify_tasks)
        .await?
        .into_iter()
        .flatten()
        .collect();

    set_progress_text("Files verified");
    set_progress_amount(1);
    set_progress_total(1);

    // Clone different chunks, if the local file is already up to date then
    // nothing will be cloned.

    for downloader in &downloaders {
        need_download_chunk_count += downloader.chunk_download_count();
    }

    let mut download_tasks: Vec<tokio::task::JoinHandle<anyhow::Result<()>>> = Vec::new();
    for downloader in downloaders {
        tracing::debug!(path =? downloader.output_path().display(), size =? downloader.chunk_download_count(), "Adding download task");

        let progress_state = progress_state.clone();

        download_tasks.push(tokio::spawn(async move {
            let mut remote_chunks = pin!(downloader.clone_remote_chunks().await);
            while let Some(chunk_size) = remote_chunks.next().await {
                let _chunk_size = chunk_size?;
                progress_state
                    .progress_amount
                    .fetch_add(1, Ordering::SeqCst);
            }

            Ok(())
        }));
    }

    tracing::info!("Downloading remote chunks");
    set_progress_text("Downloading files");
    set_progress_amount(0);
    set_progress_total(need_download_chunk_count);

    try_join_all(download_tasks).await?;

    set_progress_text("Files downloaded");
    set_progress_amount(1);
    set_progress_total(1);

    // Save manifest
    tracing::info!(path =? &local_manifest_path.display(), "Saving local manifest");

    let local_manifest = LocalManifest::from(&remote_manifest);
    save_local_manifest(&local_manifest, &local_manifest_path).await?;

    tracing::info!("Game Updated");
    set_progress_text("Game up-to-date");

    progress_state
        .update_complete
        .store(true, Ordering::Relaxed);

    Ok(())
}

async fn save_local_manifest(manifest: &LocalManifest, manifest_path: &Path) -> anyhow::Result<()> {
    if let Some(manifest_parent_dir) = manifest_path.parent() {
        std::fs::create_dir_all(manifest_parent_dir)?;
    }

    let manifest_file = std::fs::File::create(manifest_path)?;
    serde_json::to_writer(BufWriter::new(manifest_file), &manifest)?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let log_level = if args.debug {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };

    let filter = EnvFilter::builder()
        .with_default_directive(log_level.into())
        .from_env_lossy();

    let subscriber = FmtSubscriber::builder().with_env_filter(filter).finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("Critical failure: Failed to set default tracing subscriber");

    let options = eframe::NativeOptions {
        initial_window_size: Some(egui::vec2(800.0, 630.0)),
        icon_data: Some(eframe::IconData::try_from_png_bytes(ICON_BYTES)?),
        resizable: false,
        decorated: false,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    let app = UpdaterApp::new(args)?;

    eframe::run_native(
        "ROSE Online Updater",
        options,
        Box::new(|cc| {
            app.setup(cc);
            Box::new(app)
        }),
    )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    Ok(())
}
