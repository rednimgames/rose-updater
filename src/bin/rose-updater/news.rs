//! Native rendering of roseonlinegame.com/launcher.html (no HTML engine).
//!
//! Geometry, colors, typography and the two/three-card layout follow the site's
//! launcher.css. Keep the full 520px page visible, including the bottom dates.

use std::{cell::RefCell, io::Cursor as IoCursor, rc::Rc, time::Duration};

use anyhow::{bail, Context};
use directories::ProjectDirs;
use fltk::{
    app, button::Button, draw, enums::*, frame::Frame, group::Group, image::RgbImage, prelude::*,
};
use futures::{stream, StreamExt};
use image::{imageops::FilterType, ImageReader};
use reqwest::{Client, Url};
use serde::Deserialize;
use tracing::warn;

use super::Message;

const NEWS_URL: &str = "https://www.roseonlinegame.com/api/v1/news";
const MAX_NEWS_BYTES: usize = 2 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const CARD_COUNT: usize = 5;
const REGULAR: Font = Font::Screen;
const BOLD: Font = Font::ScreenBold;

#[derive(Debug, Deserialize)]
struct Feed {
    data: Vec<Article>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Article {
    title: String,
    category: Category,
    #[serde(default)]
    image: Option<String>,
    published_at: String,
    link: String,
}

#[derive(Debug, Deserialize)]
struct Category {
    title: String,
}

#[derive(Debug)]
pub(super) enum NewsMessage {
    Articles(Vec<Article>),
    Image { index: usize, pixels: Vec<u8> },
}

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

fn card_rect(index: usize) -> Rect {
    if index < 2 {
        Rect {
            x: 10 + index as i32 * 385,
            y: 99,
            w: 375,
            h: 242,
        }
    } else {
        let column = (index - 2) as i32;
        // Round each CSS grid boundary independently, avoiding a gap at the right edge.
        let left = (column * 770 + 1) / 3;
        let right = ((column + 1) * 770 + 1) / 3 - 10;
        Rect {
            x: 10 + left,
            y: 351,
            w: right - left,
            h: 159,
        }
    }
}

/// Titles use the site's regular face: the CSS requests 500 but only loads 400/700.
fn wrap_title(text: &str, width: i32, font_size: i32) -> Vec<String> {
    draw::set_font(REGULAR, font_size);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && draw::width(&candidate) > f64::from(width) {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Use the publication date from the API, not the viewer's local timezone.
fn publication_date(value: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let Some(date) = value.get(..10) else {
        return String::new();
    };
    let mut parts = date.split('-');
    let parsed = (
        parts.next().and_then(|s| s.parse::<u32>().ok()),
        parts.next().and_then(|s| s.parse::<usize>().ok()),
        parts.next().and_then(|s| s.parse::<u32>().ok()),
    );
    match parsed {
        (Some(year), Some(month @ 1..=12), Some(day @ 1..=31)) => {
            format!("{} {day}, {year}", MONTHS[month - 1])
        }
        _ => String::new(),
    }
}

fn web_url(value: &str) -> Option<Url> {
    Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "https" | "http") && url.host_str().is_some())
}

fn parse_articles(bytes: &[u8]) -> anyhow::Result<Vec<Article>> {
    let mut feed: Feed = serde_json::from_slice(bytes)?;
    feed.data.truncate(CARD_COUNT);
    Ok(feed.data)
}

async fn fetch_bytes(client: &Client, url: Url, limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        bail!("News response exceeds size limit");
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if data.len() + chunk.len() > limit {
            bail!("News response exceeds size limit");
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

fn decode_image(bytes: &[u8]) -> anyhow::Result<image::DynamicImage> {
    let mut reader = ImageReader::new(IoCursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// CSS object-fit: cover, a 150px black gradient, and the 2px white top highlight.
/// Retain two physical pixels per logical pixel for Retina/high-DPI displays.
fn card_pixels(bytes: &[u8], width: i32, height: i32) -> anyhow::Result<Vec<u8>> {
    let mut image = decode_image(bytes)?
        .resize_to_fill(width as u32 * 2, height as u32 * 2, FilterType::Triangle)
        .to_rgba8();
    let height = image.height();
    for (_, y, pixel) in image.enumerate_pixels_mut() {
        let shade = ((height as f32 - y as f32 - 0.5) / 300.0).min(1.0);
        for channel in &mut pixel.0[..3] {
            let mut value = f32::from(*channel) * shade;
            if y < 4 {
                value = value * 0.8 + 255.0 * 0.2;
            }
            *channel = value.round() as u8;
        }
    }
    Ok(image.into_raw())
}

fn rgb_image(pixels: &[u8], width: i32, height: i32) -> anyhow::Result<RgbImage> {
    let mut image = RgbImage::new(pixels, width * 2, height * 2, ColorDepth::Rgba8)?;
    image.scale(width, height, false, true);
    Ok(image)
}

fn bundled_image(bytes: &[u8]) -> anyhow::Result<RgbImage> {
    let image = decode_image(bytes)?.to_rgba8();
    Ok(RgbImage::new(
        image.as_raw(),
        image.width() as i32,
        image.height() as i32,
        ColorDepth::Rgba8,
    )?)
}

fn load_fonts() -> anyhow::Result<()> {
    let directory = ProjectDirs::from("com", "Rednim Games", "ROSE Online")
        .context("Unable to locate the launcher cache")?
        .cache_dir()
        .join("news-fonts-v2-001");
    std::fs::create_dir_all(&directory)?;
    for (name, bytes, slot) in [
        (
            "ROSENews-Regular.ttf",
            include_bytes!("../../../res/ROSENews-Regular.ttf").as_slice(),
            REGULAR,
        ),
        (
            "ROSENews-Bold.ttf",
            include_bytes!("../../../res/ROSENews-Bold.ttf").as_slice(),
            BOLD,
        ),
    ] {
        let path = directory.join(name);
        // FLTK requires a filename. The embedded copies keep news independent of a font CDN.
        if std::fs::read(&path).ok().as_deref() != Some(bytes) {
            std::fs::write(&path, bytes)?;
        }
        let font = Font::load_font(&path)?;
        // CoreText accepts the full face name. GDI and Xft/Pango use FLTK's
        // style prefix plus the family name; a full "... Bold" face name can
        // silently select a system fallback on Windows.
        let font = if cfg!(target_os = "macos") {
            font
        } else if slot == BOLD {
            "BROSE News".to_owned()
        } else {
            " ROSE News".to_owned()
        };
        Font::set_font(slot, &font);
    }
    Ok(())
}

pub(super) async fn load(sender: app::Sender<Message>) {
    if let Err(error) = load_feed(sender).await {
        // News is optional; never send a fatal updater error for a website failure.
        warn!(%error, "Unable to load launcher news");
    }
}

async fn load_feed(sender: app::Sender<Message>) -> anyhow::Result<()> {
    let client = Client::builder().timeout(Duration::from_secs(15)).build()?;
    let bytes = fetch_bytes(&client, Url::parse(NEWS_URL)?, MAX_NEWS_BYTES).await?;
    let articles = parse_articles(&bytes)?;
    let images: Vec<_> = articles
        .iter()
        .enumerate()
        .filter_map(|(index, article)| {
            article
                .image
                .as_deref()
                .and_then(web_url)
                .map(|url| (index, url))
        })
        .collect();
    sender.send(Message::News(NewsMessage::Articles(articles)));
    let mut downloads = stream::iter(images)
        .map(|(index, url)| {
            let client = client.clone();
            async move {
                let result = async {
                    let bytes = fetch_bytes(&client, url, MAX_IMAGE_BYTES).await?;
                    let rect = card_rect(index);
                    tokio::task::spawn_blocking(move || card_pixels(&bytes, rect.w - 2, rect.h - 2))
                        .await?
                }
                .await;
                (index, result)
            }
        })
        .buffer_unordered(CARD_COUNT);
    while let Some((index, result)) = downloads.next().await {
        match result {
            Ok(pixels) => sender.send(Message::News(NewsMessage::Image { index, pixels })),
            Err(error) => warn!(index, %error, "Unable to load news image"),
        }
    }
    Ok(())
}

struct Card {
    article: Article,
    image: Option<RgbImage>,
    lines: Vec<String>,
    large: bool,
    hover: f64,
    target_hover: bool,
    hover_started: std::time::Instant,
    hover_from: f64,
    animating: bool,
}

// Tailwind's transition-colors timing function: cubic-bezier(0.4, 0, 0.2, 1).
fn hover_easing(progress: f64) -> f64 {
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..16 {
        let t = (low + high) / 2.0;
        let x = 1.2 * (1.0 - t) * (1.0 - t) * t + 0.6 * (1.0 - t) * t * t + t * t * t;
        if x < progress {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) / 2.0;
    t * t * (3.0 - 2.0 * t)
}

impl Card {
    fn title_rect(&self, button: &Button) -> Rect {
        let line_height = if self.large { 24 } else { 26 };
        let date_height = if self.large { 18 } else { 16 };
        Rect {
            x: button.x() + 21,
            y: button.y() + button.h()
                - 1
                - 16
                - date_height
                - 8
                - self.lines.len() as i32 * line_height,
            w: button.w() - 42,
            h: self.lines.len() as i32 * line_height,
        }
    }

    fn title_contains(&self, button: &Button, x: i32, y: i32) -> bool {
        let rect = self.title_rect(button);
        if !rect.contains(x, y) {
            return false;
        }
        let line_height = if self.large { 24 } else { 26 };
        let index = ((y - rect.y) / line_height) as usize;
        draw::set_font(REGULAR, if self.large { 18 } else { 17 });
        self.lines
            .get(index)
            .is_some_and(|line| f64::from(x - rect.x) < draw::width(line))
    }

    fn draw(&mut self, button: &Button) {
        let (x, y, w, h) = (button.x(), button.y(), button.w(), button.h());
        draw::push_clip(x, y, w, h);
        draw::draw_rect_fill(x, y, w, h, Color::from_rgb(31, 41, 55));
        if let Some(image) = &mut self.image {
            image.draw(x + 1, y + 1, w - 2, h - 2);
        }
        let text_x = x + 21;
        let category = self.article.category.title.to_uppercase();
        let font_size = if self.large { 12 } else { 8 };
        let padding = if self.large { 12 } else { 10 };
        let badge_y = y + if self.large { 21 } else { 16 };
        draw::set_font(BOLD, font_size);
        let badge_width = draw::width(&category).ceil() as i32 + padding * 2 + 2;
        draw::draw_rect_fill(
            text_x,
            badge_y,
            badge_width,
            font_size + 12,
            Color::from_rgb(220, 38, 38),
        );
        draw::set_draw_color(Color::from_rgb(248, 113, 113));
        draw::draw_rect(text_x, badge_y, badge_width, font_size + 12);
        draw::set_draw_color(Color::White);
        draw::draw_text(&category, text_x + padding + 1, badge_y + 6 + font_size);

        let rect = self.title_rect(button);
        let size = if self.large { 18 } else { 17 };
        let line_height = if self.large { 24 } else { 26 };
        draw::set_font(REGULAR, size);
        let hover = self.hover;
        let color = Color::from_rgb(
            (255.0 - 13.0 * hover) as u8,
            (255.0 - 207.0 * hover) as u8,
            (255.0 - 242.0 * hover) as u8,
        );
        for (index, line) in self.lines.iter().enumerate() {
            let baseline = rect.y + index as i32 * line_height + if self.large { 17 } else { 19 };
            draw::set_draw_color(Color::from_rgb(30, 29, 39));
            draw::draw_text(line, text_x + 1, baseline + 1);
            draw::set_draw_color(color);
            draw::draw_text(line, text_x, baseline);
        }
        let date_y = y + h - 1 - 16;
        draw::set_font(BOLD, if self.large { 16 } else { 14 });
        draw::set_draw_color(Color::from_rgb(156, 171, 185));
        draw::draw_text(
            &publication_date(&self.article.published_at),
            text_x + 22,
            date_y - 3,
        );
        draw::set_draw_color(Color::from_rgb(242, 48, 13));
        draw::set_line_style(draw::LineStyle::Solid, 1);
        draw::draw_arc(text_x + 2, date_y - 15, 12, 12, 0.0, 360.0);
        draw::draw_line(text_x + 8, date_y - 13, text_x + 8, date_y - 9);
        draw::draw_line(text_x + 8, date_y - 9, text_x + 11, date_y - 9);
        draw::set_line_style(draw::LineStyle::Solid, 0);
        if button.has_focus() && button.has_visible_focus() {
            draw::set_draw_color(Color::White);
            draw::draw_focus_rect(x + 1, y + 1, w - 2, h - 2);
        }
        draw::pop_clip();
    }
}

fn set_hover(button: &mut Button, card: &Rc<RefCell<Card>>, hovered: bool) {
    let mut state = card.borrow_mut();
    if state.target_hover == hovered {
        return;
    }
    state.hover_started = std::time::Instant::now();
    state.hover_from = state.hover;
    state.target_hover = hovered;
    if state.animating || (state.hover - f64::from(hovered)).abs() < f64::EPSILON {
        return;
    }
    state.animating = true;
    drop(state);
    let card = card.clone();
    let mut button = button.clone();
    app::add_timeout3(0.0, move |handle| {
        let mut state = card.borrow_mut();
        let elapsed = (state.hover_started.elapsed().as_secs_f64() / 0.15).min(1.0);
        let to = f64::from(state.target_hover);
        state.hover = if elapsed == 1.0 {
            to
        } else {
            state.hover_from + (to - state.hover_from) * hover_easing(elapsed)
        };
        button.redraw();
        if elapsed < 1.0 {
            app::repeat_timeout3(1.0 / 60.0, handle);
        } else {
            state.animating = false;
        }
    });
}

pub(super) struct NewsPanel {
    pub group: Group,
    cards: Vec<(Button, Rc<RefCell<Card>>)>,
}

impl NewsPanel {
    pub fn new() -> Self {
        if let Err(error) = load_fonts() {
            warn!(%error, "Unable to load bundled news fonts");
            Font::set_font(REGULAR, &app::get_font(Font::Helvetica));
            Font::set_font(BOLD, &app::get_font(Font::HelveticaBold));
        }
        let group = Group::new(0, 0, 780, 530, None);
        let mut background = Frame::new(0, 0, 780, 520, None);
        let mut art = bundled_image(include_bytes!("../../../res/news-background.webp"))
            .expect("valid bundled news background");
        let mut logo = bundled_image(include_bytes!("../../../res/news-logo.webp"))
            .expect("valid bundled news logo");
        logo.scale(130, 80, false, true);
        background.draw(move |frame| {
            draw::push_clip(frame.x(), frame.y(), frame.w(), frame.h());
            draw::draw_rect_fill(
                frame.x(),
                frame.y(),
                frame.w(),
                frame.h(),
                Color::from_rgb(62, 51, 62),
            );
            art.draw(frame.x(), frame.y(), frame.w(), frame.h());
            logo.draw(frame.x() + 325, frame.y() + 10, 130, 80);
            draw::pop_clip();
        });
        group.end();
        Self {
            group,
            cards: Vec::new(),
        }
    }

    pub fn update(&mut self, message: NewsMessage) {
        match message {
            NewsMessage::Articles(articles) => {
                self.group.begin();
                for (index, article) in articles.into_iter().take(CARD_COUNT).enumerate() {
                    let rect = card_rect(index);
                    let large = index < 2;
                    let lines =
                        wrap_title(&article.title, rect.w - 42, if large { 18 } else { 17 });
                    let link = web_url(&article.link);
                    let card = Rc::new(RefCell::new(Card {
                        article,
                        lines,
                        large,
                        image: None,
                        hover: 0.0,
                        target_hover: false,
                        hover_started: std::time::Instant::now(),
                        hover_from: 0.0,
                        animating: false,
                    }));
                    let mut button = Button::new(rect.x, rect.y, rect.w, rect.h, None);
                    button.set_label(&card.borrow().article.title);
                    // Keep the semantic label, but paint it only in our card renderer.
                    button.set_label_type(LabelType::None);
                    button.set_align(Align::Inside | Align::Center);
                    button.set_frame(FrameType::NoBox);
                    button.draw({
                        let card = card.clone();
                        move |button| card.borrow_mut().draw(button)
                    });
                    button.handle({
                        let card = card.clone();
                        let clickable = link.is_some();
                        move |button, event| match event {
                            Event::Enter | Event::Move | Event::Leave => {
                                if let Some(mut window) = button.window() {
                                    window.set_cursor(if event != Event::Leave && clickable {
                                        Cursor::Hand
                                    } else {
                                        Cursor::Default
                                    });
                                }
                                let hovered = event != Event::Leave
                                    && card.borrow().title_contains(
                                        button,
                                        app::event_x(),
                                        app::event_y(),
                                    );
                                set_hover(button, &card, hovered);
                                true
                            }
                            Event::KeyDown if app::event_key() == Key::Enter => {
                                button.do_callback();
                                true
                            }
                            Event::Push | Event::Released
                                if app::event_mouse_button() != app::MouseButton::Left =>
                            {
                                true
                            }
                            _ => false,
                        }
                    });
                    button.set_callback(move |_| {
                        if let Some(link) = &link {
                            if let Err(error) = open::that(link.as_str()) {
                                warn!(%error, "Unable to open news link");
                            }
                        }
                    });
                    self.cards.push((button, card));
                }
                self.group.end();
            }
            NewsMessage::Image { index, pixels } => {
                if let Some((button, card)) = self.cards.get_mut(index) {
                    match rgb_image(&pixels, button.w() - 2, button.h() - 2) {
                        Ok(image) => card.borrow_mut().image = Some(image),
                        Err(error) => warn!(%error, "Unable to display news image"),
                    }
                }
            }
        }
        self.group.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_articles_do_not_depend_on_descriptions_or_pagination() {
        let feed: Feed = serde_json::from_str(r#"{"data":[{"title":"Patch & news","category":{"title":"Development","id":3},"published_at":"2026-10-16T18:21:38.000000Z","link":"https://www.roseonlinegame.com/news/281"}],"links":{},"meta":{}}"#).unwrap();
        assert_eq!(feed.data[0].title, "Patch & news");
        assert!(feed.data[0].image.is_none());
    }

    #[test]
    fn dates_keep_the_publication_day_and_reject_bad_values() {
        assert_eq!(
            publication_date("2026-09-01T00:01:00.000000Z"),
            "September 1, 2026"
        );
        for invalid in ["", "bad date", "2026-99-01", "2026-01-00", "💐💐💐"] {
            assert_eq!(publication_date(invalid), "");
        }
    }

    #[test]
    fn news_links_only_open_web_urls() {
        assert!(web_url("https://www.roseonlinegame.com/news/1").is_some());
        for url in [
            "file:///tmp/test",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "invalid",
        ] {
            assert!(web_url(url).is_none());
        }
    }

    #[test]
    fn corrupt_images_are_nonfatal() {
        assert!(card_pixels(b"not an image", 373, 240).is_err());
    }

    #[test]
    fn feed_keeps_the_first_five_articles_in_server_order() {
        let data: Vec<_> = (0..8).map(|i| serde_json::json!({
            "title": i.to_string(), "category": {"title": "News"},
            "published_at": "2026-01-01T12:00:00Z", "link": "https://www.roseonlinegame.com/news"
        })).collect();
        let articles =
            parse_articles(&serde_json::to_vec(&serde_json::json!({"data":data})).unwrap())
                .unwrap();
        assert_eq!(
            articles
                .iter()
                .map(|a| a.title.as_str())
                .collect::<Vec<_>>(),
            ["0", "1", "2", "3", "4"]
        );
        assert!(parse_articles(b"<html>Maintenance</html>").is_err());
        assert!(parse_articles(br#"{"data":[]}"#).unwrap().is_empty());
    }

    #[test]
    fn thumbnails_cover_the_card_and_preserve_the_gradient() {
        let image = image::RgbImage::from_pixel(40, 80, image::Rgb([200, 100, 50]));
        let mut png = IoCursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let pixels = card_pixels(png.get_ref(), 373, 240).unwrap();
        assert_eq!(pixels.len(), 746 * 480 * 4);
        assert_eq!(&pixels[4 * 746 * 4..4 * 746 * 4 + 4], &[200, 100, 50, 255]);
        assert!(pixels[pixels.len() - 4] < 2);
        assert_eq!(pixels[pixels.len() - 1], 255);
    }

    #[tokio::test]
    async fn news_http_failures_and_large_responses_are_rejected() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        for response in [
            "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n01234567890123456789",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n14\r\n01234567890123456789\r\n0\r\n\r\n",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(socket);
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap() > 0 {
                    if line == "\r\n" { break; }
                    line.clear();
                }
                reader.get_mut().write_all(response.as_bytes()).await.unwrap();
            });
            let client = Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap();
            assert!(fetch_bytes(&client, url, 10).await.is_err());
            server.await.unwrap();
        }
    }
}
