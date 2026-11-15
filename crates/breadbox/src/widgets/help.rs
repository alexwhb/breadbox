//! Help → the bundled documentation in a window (`APP-4`): the Breadbox guide (README), one
//! page per screen (`docs/guide/*.md`, opened by each screen's "?" button), the Plugin SDK and
//! the lesson format, rendered by the Markdown widget from the app resources
//! (`Contents/Resources/docs` in the bundle, the repository in development).
//!
//! UI text stays short (see `docs/UI-TEXT.md`); anything that needs a paragraph lives here.

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
    /// `docs/guide/workbench.md`: bench, wiring, world, instruments, probes, serial.
    Workbench,
    /// `docs/guide/firmware.md`: sketches, Verify / Upload, debugging, radios.
    Firmware,
    /// `docs/guide/pro.md`: netlist, analyses, schematic, PCB.
    Pro,
    /// `docs/guide/plugins.md`: catalog, permissions, instruments, local plugins.
    Plugins,
    /// `docs/guide/lessons.md`: tracks, math mode, the lesson editor.
    Lessons,
    PluginSdk,
    LessonFormat,
}

impl HelpTopic {
    pub const ALL: [HelpTopic; 8] = [
        HelpTopic::Guide,
        HelpTopic::Workbench,
        HelpTopic::Firmware,
        HelpTopic::Pro,
        HelpTopic::Plugins,
        HelpTopic::Lessons,
        HelpTopic::PluginSdk,
        HelpTopic::LessonFormat,
    ];

    pub fn title(self) -> &'static str {
        match self {
            HelpTopic::Guide => "Getting started",
            HelpTopic::Workbench => "Workbench",
            HelpTopic::Firmware => "Firmware",
            HelpTopic::Pro => "Pro mode",
            HelpTopic::Plugins => "Plugins",
            HelpTopic::Lessons => "Lessons",
            HelpTopic::PluginSdk => "Plugin SDK",
            HelpTopic::LessonFormat => "Lesson format",
        }
    }

    /// Where the document lives in the resources (bundle first, then the repository layout).
    fn files(self) -> &'static [&'static str] {
        match self {
            HelpTopic::Guide => &["docs/README.md", "README.md"],
            HelpTopic::Workbench => &["docs/guide/workbench.md"],
            HelpTopic::Firmware => &["docs/guide/firmware.md"],
            HelpTopic::Pro => &["docs/guide/pro.md"],
            HelpTopic::Plugins => &["docs/guide/plugins.md"],
            HelpTopic::Lessons => &["docs/guide/lessons.md"],
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
    // below the top bar (its "?" stays clickable) and never wider than the screen
    let screen = ctx.content_rect();
    let area = egui::Rect::from_min_max(
        egui::pos2(screen.left(), screen.top() + crate::theme::size::HEADER),
        screen.max,
    )
    .shrink(12.0);
    let size = vec2(760.0f32.min(area.width()), 620.0f32.min(area.height()));
    egui::Window::new("Breadbox Help")
        .id(Id::unique("help-window"))
        .open(&mut open)
        .default_rect(egui::Rect::from_center_size(area.center(), size))
        .min_size(vec2(
            480.0f32.min(area.width()),
            320.0f32.min(area.height()),
        ))
        .max_size(area.size())
        .constrain_to(area)
        .collapsible(false)
        .resizable(true)
        .show(ctx, |ui| {
            let p = pal(ui);
            ui.horizontal_top(|ui| {
                // topic list on the left (eight topics don't fit a segmented control)
                ui.vertical(|ui| {
                    ui.set_width(160.0);
                    for t in HelpTopic::ALL {
                        let r = widgets::menu_item(ui, t.title(), t == topic, true);
                        if r {
                            topic = t;
                        }
                    }
                });
                ui.separator();
                // the page's width: the window's first (sizing) pass has no width limit, and
                // the Markdown widget lays out to the width it is given
                let doc_w = if ui.is_sizing_pass() {
                    size.x - 200.0
                } else {
                    ui.available_width()
                };
                egui::ScrollArea::vertical()
                    .id_salt(("help-scroll", topic))
                    .max_width(doc_w)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // top-down: the scroll area would inherit the row's left-to-right
                        ui.vertical(|ui| {
                            ui.set_max_width(doc_w - 14.0);
                            ui.style_mut().override_font_id = Some(font::ui(13.0));
                            ui.visuals_mut().override_text_color = Some(p.text_dim);
                            markdown::show(ui, &text);
                        });
                    });
            });
        });
    app.help = open.then_some(topic);
}
