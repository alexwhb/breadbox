//! Help → the bundled documentation in a window (`APP-4`): the Breadbox guide (README), the
//! Plugin SDK and the lesson format, rendered by the Markdown widget from the app resources
//! (`Contents/Resources/docs` in the bundle, the repository in development).

use crate::app::AppCtx;
use crate::theme::{font, pal};
use crate::widgets::{self, markdown};
use eframe::egui::{self, Id, vec2};
use serde::{Deserialize, Serialize};

/// A document in the Help window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum HelpTopic {
    /// README: what Breadbox is, getting started, building.
    #[default]
    Guide,
    PluginSdk,
    LessonFormat,
}

impl HelpTopic {
    pub const ALL: [HelpTopic; 3] = [
        HelpTopic::Guide,
        HelpTopic::PluginSdk,
        HelpTopic::LessonFormat,
    ];

    pub fn title(self) -> &'static str {
        match self {
            HelpTopic::Guide => "Breadbox guide",
            HelpTopic::PluginSdk => "Plugin SDK",
            HelpTopic::LessonFormat => "Lesson format",
        }
    }

    /// Where the document lives in the resources (bundle first, then the repository layout).
    fn files(self) -> &'static [&'static str] {
        match self {
            HelpTopic::Guide => &["docs/README.md", "README.md"],
            HelpTopic::PluginSdk => &["docs/PLUGIN-SDK.md"],
            HelpTopic::LessonFormat => &["docs/LESSON-FORMAT.md"],
        }
    }
}

/// The document's Markdown, ready for the widget (images and raw HTML lines dropped: the
/// window shows text).
pub fn text(app: &AppCtx, topic: HelpTopic) -> String {
    let raw = topic
        .files()
        .iter()
        .find_map(|f| app.resources.file(f))
        .and_then(|p| std::fs::read_to_string(p).ok());
    match raw {
        Some(t) => t
            .lines()
            .filter(|l| {
                let l = l.trim_start();
                !(l.starts_with("![") || l.starts_with('<'))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => format!(
            "# {}\n\nThis document is not bundled with this build.",
            topic.title()
        ),
    }
}

/// Draws the Help window when one is open (`AppCtx::help`).
pub fn show(ctx: &egui::Context, app: &mut AppCtx) {
    let Some(mut topic) = app.help else {
        return;
    };
    let mut open = true;
    // read once per topic, not every frame
    static CACHE: std::sync::Mutex<Option<(HelpTopic, String)>> = std::sync::Mutex::new(None);
    let text = {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        match c.as_ref() {
            Some((t, s)) if *t == topic => s.clone(),
            _ => {
                let s = markdown::displayable(ctx, &text(app, topic));
                *c = Some((topic, s.clone()));
                s
            }
        }
    };
    egui::Window::new("Breadbox Help")
        .id(Id::unique("help-window"))
        .open(&mut open)
        .default_size(vec2(760.0, 620.0))
        .min_size(vec2(480.0, 320.0))
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            let p = pal(ui);
            ui.horizontal(|ui| {
                let segs: Vec<widgets::Seg<HelpTopic>> = HelpTopic::ALL
                    .iter()
                    .map(|t| widgets::Seg::new(*t, t.title()))
                    .collect();
                widgets::segmented(ui, "Help topic", &mut topic, &segs);
            });
            ui.add_space(6.0);
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt(("help-scroll", topic))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.style_mut().override_font_id = Some(font::ui(13.0));
                    ui.visuals_mut().override_text_color = Some(p.text_dim);
                    markdown::show(ui, &text);
                });
        });
    app.help = open.then_some(topic);
}
