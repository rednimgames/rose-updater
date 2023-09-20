use eframe::egui;

use crate::style::*;

pub enum NewsLabel<'a> {
    News,
    Development,
    Maintenance,
    Custom(&'a str),
}

impl<'a> egui::Widget for NewsLabel<'a> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let (text, color, stroke) = match self {
            NewsLabel::News => ("News", RED_600, egui::Stroke::new(1.0, RED_700)),
            NewsLabel::Development => ("Development", SKY_700, egui::Stroke::new(1.0, SKY_600)),
            NewsLabel::Maintenance => ("Maintenance", GRAY_700, egui::Stroke::new(1.0, GRAY_600)),
            NewsLabel::Custom(name) => (name, GRAY_700, egui::Stroke::new(1.0, GRAY_600)),
        };

        let desired_size = egui::vec2(112.0, 24.0);

        let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, text));

        if ui.is_rect_visible(rect) {
            // Draw the frame
            ui.painter().rect(
                rect.expand2(egui::vec2(1.0, 1.0)),
                egui::Rounding::ZERO,
                color,
                stroke,
            );

            // Draw text inside the frame
            let mut inner_ui = ui.child_ui(
                rect,
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
            );
            inner_ui.label(
                egui::RichText::new(text)
                    .size(12.0)
                    .color(egui::Color32::WHITE)
                    .family(egui::FontFamily::Name(FONT_POPPINS_MEDIUM.into())),
            );
        }

        response
    }
}
