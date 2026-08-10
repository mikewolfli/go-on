use std::mem;
use std::{collections::HashMap, collections::HashSet};

use crate::i18n::{I18n, Lang};
use serde::{Deserialize, Serialize};

// ── Data types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    pub title: String,
    pub description: String,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCategory {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub templates: Vec<PromptTemplate>,
}

/// Wrapper for en.json which has a "categories" key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromptCollectionEn {
    categories: Vec<PromptCategory>,
}

// ── View state ────────────────────────────────────────────────────────────

pub struct PromptsView {
    /// Loaded collection of prompt categories
    pub collection: Vec<PromptCategory>,
    /// Search query for filtering templates
    pub search_query: String,
    /// Currently selected category ID
    pub selected_category: Option<String>,
    /// Currently selected template for detail view
    pub selected_template: Option<PromptTemplate>,
    /// Edit mode: editing an existing custom template
    pub edit_mode: bool,
    pub edit_template: Option<PromptTemplate>,
    /// Show create custom template dialog
    pub show_create: bool,
    pub create_template: PromptTemplate,
    /// Error message
    pub error: String,
    /// Whether the view has been loaded
    pub loaded: bool,
    /// Stored prompt templates for the chat `/` command expansion.
    /// These are derived from the loaded data.
    pub command_templates: Vec<CommandTemplate>,
    /// Tracks the last known language to detect switches.
    current_lang: Lang,
    /// Version number incremented on every reload, used to detect changes
    /// without cloning data every frame.
    pub command_version: u64,
    /// Pending insert content to be taken by the app for chat input.
    pub pending_insert: Option<String>,
}

/// A simplified prompt template for the chat `/` command lookup.
#[derive(Debug, Clone)]
pub struct CommandTemplate {
    pub command: String,
    pub content: String,
}

impl PromptsView {
    pub fn new() -> Self {
        Self {
            collection: Vec::new(),
            search_query: String::new(),
            selected_category: None,
            selected_template: None,
            edit_mode: false,
            edit_template: None,
            show_create: false,
            create_template: PromptTemplate {
                id: String::new(),
                title: String::new(),
                description: String::new(),
                content: String::new(),
                tags: Vec::new(),
            },
            error: String::new(),
            loaded: false,
            command_templates: Vec::new(),
            command_version: 0,
            current_lang: Lang::En,
            pending_insert: None,
        }
    }

    /// Reload prompt data for the given language.
    /// Returns true if data was (re)loaded.
    pub fn ensure_loaded(&mut self, lang: Lang) -> bool {
        if self.loaded {
            return false;
        }

        let filename = match lang {
            Lang::ZhCn => "zh-CN.json",
            Lang::ZhTw => "zh-TW.json",
            Lang::En => "en.json",
        };

        // Look for prompts file alongside the binary or in CWD
        let path = Self::prompts_base_dir().join(filename);

        let mut loaded_main = false;
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match Self::parse_prompts(&content) {
                    Ok(categories) => {
                        self.collection = categories;
                        loaded_main = true;
                    }
                    Err(e) => {
                        self.error = format!("Failed to parse prompts file: {}", e);
                    }
                },
                Err(e) => {
                    self.error = format!("Failed to read prompts file: {}", e);
                }
            }
        }

        if !loaded_main {
            self.error = format!("Prompts file not found: {}", path.display());
        }

        // Merge custom templates from prompts/custom/{lang}.json
        if loaded_main {
            if lang != Lang::En {
                let fallback_path = Self::prompts_base_dir().join("en.json");
                if fallback_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&fallback_path) {
                        if let Ok(fallback_categories) = Self::parse_prompts(&content) {
                            Self::merge_with_fallback(&mut self.collection, fallback_categories);
                        }
                    }
                }
            }

            self.merge_custom_templates(lang);
            // Derive command templates from loaded data
            self.command_templates = self.derive_command_templates();
            // Select first category if none selected
            if self.selected_category.is_none() && !self.collection.is_empty() {
                self.selected_category = Some(self.collection[0].id.clone());
            }
            self.current_lang = lang;
            self.loaded = true;
            return true;
        }

        false
    }

    /// Parse prompts JSON content into a Vec<PromptCategory>.
    /// Handles both the en.json format ({"categories": [...]}) and the
    /// zh-*.json format ([...]).
    fn parse_prompts(content: &str) -> Result<Vec<PromptCategory>, String> {
        // Try the en.json format first (object with "categories" key)
        if let Ok(wrapper) = serde_json::from_str::<PromptCollectionEn>(content) {
            return Ok(wrapper.categories);
        }

        // Try the array format (zh-*.json)
        if let Ok(categories) = serde_json::from_str::<Vec<PromptCategory>>(content) {
            return Ok(categories);
        }

        Err("Invalid prompts JSON: expected an array of categories or an object with a 'categories' key".to_string())
    }

    fn merge_with_fallback(primary: &mut Vec<PromptCategory>, fallback: Vec<PromptCategory>) {
        for fallback_cat in fallback {
            if let Some(primary_cat) = primary.iter_mut().find(|c| c.id == fallback_cat.id) {
                for fallback_tmpl in fallback_cat.templates {
                    if primary_cat
                        .templates
                        .iter()
                        .all(|t| t.id != fallback_tmpl.id)
                    {
                        primary_cat.templates.push(fallback_tmpl);
                    }
                }
            } else {
                primary.push(fallback_cat);
            }
        }
    }

    /// Derive command templates from the category collection.
    /// Each template generates a command "/{category_id}.{template_id}" or "/{template_id}".
    fn derive_command_templates(&self) -> Vec<CommandTemplate> {
        let mut result = Vec::new();
        let mut short_id_counts: HashMap<&str, usize> = HashMap::new();
        let mut seen_commands: HashSet<String> = HashSet::new();

        // Count id occurrences to avoid ambiguous short commands.
        for category in &self.collection {
            for template in &category.templates {
                *short_id_counts.entry(&template.id).or_insert(0) += 1;
            }
        }

        for category in &self.collection {
            for template in &category.templates {
                // Scoped command: /category_id.template_id
                let scoped = format!("/{}.{}", category.id, template.id);
                if seen_commands.insert(scoped.clone()) {
                    result.push(CommandTemplate {
                        command: scoped,
                        content: template.content.clone(),
                    });
                }

                // Short command is only generated when the template id is unique.
                if short_id_counts
                    .get(template.id.as_str())
                    .copied()
                    .unwrap_or(0)
                    == 1
                {
                    let short = format!("/{}", template.id);
                    if seen_commands.insert(short.clone()) {
                        result.push(CommandTemplate {
                            command: short,
                            content: template.content.clone(),
                        });
                    }
                }
            }
        }
        result
    }

    /// Merge custom templates from prompts/custom/{lang}.json into the collection.
    fn merge_custom_templates(&mut self, lang: Lang) {
        let custom_filename = match lang {
            Lang::ZhCn => "zh-CN.json",
            Lang::ZhTw => "zh-TW.json",
            Lang::En => "en.json",
        };

        // Look for custom prompts alongside the binary or in CWD
        let base_dir = Self::prompts_base_dir();
        let custom_path = base_dir.join("custom").join(custom_filename);

        let mut custom_templates: Vec<PromptTemplate> = Vec::new();
        if custom_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&custom_path) {
                if let Ok(templates) = serde_json::from_str::<Vec<PromptTemplate>>(&content) {
                    custom_templates = templates;
                }
            }
        }

        if custom_templates.is_empty() {
            return;
        }

        // Add or merge into the "custom" category
        if let Some(custom_cat) = self.collection.iter_mut().find(|c| c.id == "custom") {
            // Merge/replace existing custom templates by id
            for ct in custom_templates {
                if let Some(existing) = custom_cat.templates.iter_mut().find(|t| t.id == ct.id) {
                    *existing = ct;
                } else {
                    custom_cat.templates.push(ct);
                }
            }
        } else {
            let category_name = match lang {
                Lang::ZhCn => "自定义",
                Lang::ZhTw => "自訂",
                Lang::En => "Custom",
            };
            self.collection.push(PromptCategory {
                id: "custom".to_string(),
                name: category_name.to_string(),
                icon: "⭐".to_string(),
                templates: custom_templates,
            });
        }
    }

    /// Reload prompts for the given language.
    pub fn reload(&mut self, lang: Lang) {
        self.loaded = false;
        self.command_templates.clear();
        self.current_lang = lang;
        self.selected_category = None;
        self.selected_template = None;
        self.command_version += 1;
        // Preserve old collection in case reload fails
        let old_collection = mem::take(&mut self.collection);
        if !self.ensure_loaded(lang) {
            // Reload failed — restore old collection so UI still shows something
            self.collection = old_collection;
            // error is already set by ensure_loaded
        }
    }

    /// Show the prompts management view.
    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        // Detect language switch and auto-reload
        if i18n.lang != self.current_lang {
            self.reload(i18n.lang);
        }

        // Ensure data is loaded
        self.ensure_loaded(i18n.lang);

        {
            ui.heading(i18n.t("prompts.title"));
            ui.separator();
            ui.add_space(4.0);

            // Error message
            if !self.error.is_empty() {
                let text = self.error.clone();
                let resp = ui.colored_label(egui::Color32::RED, &text);
                resp.context_menu(|ui| {
                    if ui.button(i18n.t("common.copyButton")).clicked() {
                        ui.ctx().copy_text(text.clone());
                        ui.close();
                    }
                });
                ui.add_space(4.0);
            }

            // ── Toolbar ─────────────────────────────────────────────────────
            ui.horizontal(|ui| {
                // Search box
                ui.label("🔍");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text(i18n.t("prompts.search"))
                        .desired_width(250.0),
                );
                if resp.changed() {
                    // Clear selection when searching
                    if !self.search_query.is_empty() {
                        self.selected_template = None;
                    }
                }

                // Reload button
                if ui.add(egui::Button::new("🔄")).clicked() {
                    self.reload(i18n.lang);
                }

                // Create new template
                if ui.add(egui::Button::new("➕")).clicked() {
                    self.show_create = !self.show_create;
                    if self.show_create {
                        self.create_template = PromptTemplate {
                            id: String::new(),
                            title: String::new(),
                            description: String::new(),
                            content: String::new(),
                            tags: Vec::new(),
                        };
                        self.edit_mode = false;
                        self.edit_template = None;
                    }
                }
            });

            ui.add_space(4.0);

            // ── Create dialog ───────────────────────────────────────────────
            if self.show_create {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(i18n.t("prompts.create"))
                            .strong()
                            .size(14.0),
                    );

                    ui.horizontal(|ui| {
                        ui.label(i18n.t("skills.create.title"));
                        ui.text_edit_singleline(&mut self.create_template.title);
                    });
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("common.description"));
                        ui.text_edit_singleline(&mut self.create_template.description);
                    });
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("prompts.content"));
                    });
                    ui.text_edit_multiline(&mut self.create_template.content);
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("prompts.tags"));
                        let mut tags_str = self.create_template.tags.join(", ");
                        ui.text_edit_singleline(&mut tags_str);
                        self.create_template.tags = tags_str
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    });

                    ui.horizontal(|ui| {
                        if ui.button(i18n.t("prompts.save")).clicked() {
                            // Generate an ID if not set
                            if self.create_template.id.is_empty() {
                                self.create_template.id = self
                                    .create_template
                                    .title
                                    .to_lowercase()
                                    .replace(char::is_whitespace, "_")
                                    .replace(|c: char| !c.is_alphanumeric() && c != '_', "");
                                if self.create_template.id.is_empty() {
                                    self.create_template.id = format!(
                                        "custom_{}",
                                        std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_nanos())
                                            .unwrap_or(0)
                                    );
                                }
                            }

                            // Store custom template in config directory
                            if let Err(err) = self.save_custom_template(&self.create_template) {
                                self.error = err;
                            } else {
                                self.show_create = false;
                                self.reload(i18n.lang);
                            }
                        }
                        if ui.button(i18n.t("prompts.cancel")).clicked() {
                            self.show_create = false;
                        }
                    });
                });
                ui.add_space(4.0);
            }

            // ── Main content: two-column layout ─────────────────────────────
            // ── Search results (shown when search query is non-empty) ──────
            let search_active = !self.search_query.is_empty();
            if search_active {
                let query = self.search_query.clone();
                let results: Vec<(String, String, PromptTemplate)> = self
                    .collection
                    .iter()
                    .flat_map(|cat| {
                        let q = query.to_lowercase();
                        cat.templates
                            .iter()
                            .filter(move |tmpl| {
                                tmpl.title.to_lowercase().contains(&q)
                                    || tmpl.description.to_lowercase().contains(&q)
                                    || tmpl.tags.iter().any(|t| t.to_lowercase().contains(&q))
                                    || tmpl.content.to_lowercase().contains(&q)
                            })
                            .map(|tmpl| (cat.id.clone(), cat.name.clone(), tmpl.clone()))
                    })
                    .collect();

                if results.is_empty() {
                    ui.label(egui::RichText::new(i18n.t("prompts.noTemplates")).weak());
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("prompts_search_results")
                        .show(ui, |ui| {
                            for (_cat_id, cat_name, tmpl) in &results {
                                let is_selected = self
                                    .selected_template
                                    .as_ref()
                                    .is_some_and(|t| t.id == tmpl.id);

                                let border_color = if is_selected {
                                    ui.style().visuals.selection.bg_fill
                                } else {
                                    egui::Color32::TRANSPARENT
                                };

                                let resp = egui::Frame::new()
                                    .fill(ui.style().visuals.extreme_bg_color)
                                    .stroke(if border_color == egui::Color32::TRANSPARENT {
                                        egui::Stroke::NONE
                                    } else {
                                        egui::Stroke::new(1.0, border_color)
                                    })
                                    .corner_radius(4.0)
                                    .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(&tmpl.title).strong());
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "📁 {}",
                                                            cat_name
                                                        ))
                                                        .weak()
                                                        .size(11.0),
                                                    );
                                                },
                                            );
                                        });
                                        ui.label(
                                            egui::RichText::new(&tmpl.description)
                                                .size(12.0)
                                                .weak(),
                                        );
                                        if !tmpl.tags.is_empty() {
                                            ui.horizontal_wrapped(|ui| {
                                                for tag in &tmpl.tags {
                                                    ui.label(
                                                        egui::RichText::new(format!("#{}", tag))
                                                            .size(10.0)
                                                            .color(
                                                                ui.style().visuals.hyperlink_color,
                                                            ),
                                                    );
                                                }
                                            });
                                        }
                                        if ui.button(i18n.t("prompts.insert")).clicked() {
                                            self.pending_insert = Some(tmpl.content.clone());
                                        }
                                    })
                                    .response;

                                if resp.clicked() {
                                    self.selected_template = Some(tmpl.clone());
                                }

                                ui.add_space(2.0);
                            }
                        });
                }
            } else {
                // ── Two-column layout ──────────────────────────────────────
                egui::Panel::left("prompts_categories")
                    .resizable(true)
                    .default_size(200.0)
                    .size_range(150.0..=350.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(i18n.t("prompts.categories"))
                                .strong()
                                .size(14.0),
                        );
                        ui.separator();
                        ui.add_space(2.0);

                        if self.collection.is_empty() {
                            ui.label(egui::RichText::new(i18n.t("prompts.noTemplates")).weak());
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("prompts_category_list")
                                .show(ui, |ui| {
                                    for cat in &self.collection {
                                        let is_selected = self
                                            .selected_category
                                            .as_ref()
                                            .is_some_and(|id| id == &cat.id);
                                        let count = cat.templates.len();

                                        let label_text =
                                            format!("{} {} ({})", cat.icon, cat.name, count);
                                        let resp = ui.selectable_label(is_selected, &label_text);
                                        if resp.clicked() {
                                            self.selected_category = Some(cat.id.clone());
                                            self.selected_template = None;
                                        }
                                    }
                                });
                        }
                    });

                // ── Right panel: templates list + detail ───────────────────
                egui::CentralPanel::default().show(ui, |ui| {
                    match self.selected_category.clone() {
                        None => {
                            ui.label(egui::RichText::new(i18n.t("prompts.noCategory")).weak());
                        }
                        Some(ref cat_id) => {
                            let category = self.collection.iter().find(|c| c.id == *cat_id);

                            if let Some(cat) = category {
                                ui.label(
                                    egui::RichText::new(format!("{} {}", cat.icon, cat.name))
                                        .strong()
                                        .size(16.0),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} {}",
                                        cat.templates.len(),
                                        i18n.t("prompts.templates")
                                    ))
                                    .weak()
                                    .size(11.0),
                                );
                                ui.separator();
                                ui.add_space(4.0);

                                if cat.templates.is_empty() {
                                    ui.label(
                                        egui::RichText::new(i18n.t("prompts.noTemplates")).weak(),
                                    );
                                } else {
                                    egui::ScrollArea::vertical()
                                        .id_salt("prompts_template_list")
                                        .show(ui, |ui| {
                                            for tmpl in &cat.templates {
                                                let is_selected = self
                                                    .selected_template
                                                    .as_ref()
                                                    .is_some_and(|t| t.id == tmpl.id);

                                                let bg = if is_selected {
                                                    ui.style()
                                                        .visuals
                                                        .selection
                                                        .bg_fill
                                                        .gamma_multiply(0.3)
                                                } else {
                                                    ui.style().visuals.extreme_bg_color
                                                };

                                                let resp = egui::Frame::new()
                                                    .fill(bg)
                                                    .corner_radius(4.0)
                                                    .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                                                    .stroke(if is_selected {
                                                        egui::Stroke::new(
                                                            1.0,
                                                            ui.style().visuals.selection.bg_fill,
                                                        )
                                                    } else {
                                                        egui::Stroke::NONE
                                                    })
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            egui::RichText::new(&tmpl.title)
                                                                .strong(),
                                                        );
                                                        ui.label(
                                                            egui::RichText::new(&tmpl.description)
                                                                .size(12.0)
                                                                .weak(),
                                                        );
                                                        if !tmpl.tags.is_empty() {
                                                            ui.horizontal_wrapped(|ui| {
                                                                for tag in &tmpl.tags {
                                                                    ui.label(
                                                                        egui::RichText::new(
                                                                            format!("#{}", tag),
                                                                        )
                                                                        .size(10.0)
                                                                        .color(
                                                                            ui.style()
                                                                                .visuals
                                                                                .hyperlink_color,
                                                                        ),
                                                                    );
                                                                }
                                                            });
                                                        }
                                                    })
                                                    .response;

                                                if resp.clicked() {
                                                    self.selected_template = Some(tmpl.clone());
                                                }

                                                ui.add_space(2.0);
                                            }
                                        });
                                }
                            } else {
                                ui.label(egui::RichText::new(i18n.t("prompts.noCategory")).weak());
                            }
                        }
                    }
                });
            }

            // ── Detail panel (shown when a template is selected) ─────────
            if self.edit_mode {
                self.show_edit_dialog(ui, i18n);
            } else if let Some(tmpl) = &self.selected_template.clone() {
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&tmpl.title).strong().size(16.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(i18n.t("prompts.edit")).clicked() {
                                self.edit_template = Some(tmpl.clone());
                                self.edit_mode = true;
                            }
                        });
                    });
                    ui.add_space(4.0);

                    // Description
                    ui.label(egui::RichText::new(&tmpl.description).size(13.0).weak());
                    ui.add_space(4.0);

                    // Tags
                    if !tmpl.tags.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{}: ", i18n.t("prompts.tags")))
                                    .size(11.0)
                                    .weak(),
                            );
                            for tag in &tmpl.tags {
                                ui.label(
                                    egui::RichText::new(format!("#{}", tag))
                                        .size(11.0)
                                        .color(ui.style().visuals.hyperlink_color),
                                );
                            }
                        });
                        ui.add_space(4.0);
                    }

                    // Content — in a scrollable, framed area with copy
                    ui.label(
                        egui::RichText::new(i18n.t("prompts.content"))
                            .size(12.0)
                            .strong(),
                    );
                    egui::Frame::dark_canvas(ui.style())
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(8i8, 6i8))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("prompt_content_scroll")
                                .max_height(200.0)
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(&tmpl.content).size(12.0).monospace(),
                                    );
                                });
                        });

                    // Copy & Insert buttons
                    ui.horizontal(|ui| {
                        if ui.button(i18n.t("common.copyButton")).clicked() {
                            ui.ctx().copy_text(tmpl.content.clone());
                        }
                        if ui.button(i18n.t("prompts.insert")).clicked() {
                            self.pending_insert = Some(tmpl.content.clone());
                        }
                    });
                });
            }
        }
    }

    /// Show edit dialog for an existing template.
    fn show_edit_dialog(&mut self, ui: &mut egui::Ui, i18n: &I18n) {
        // Clone the edit_template to avoid borrow conflicts
        let maybe_tmpl = self.edit_template.clone();
        let Some(tmpl) = maybe_tmpl else {
            self.edit_mode = false;
            return;
        };

        // Work with a local copy, we'll save it back
        let mut edit_tmpl = tmpl.clone();

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                egui::RichText::new(i18n.t("prompts.edit"))
                    .strong()
                    .size(14.0),
            );

            ui.horizontal(|ui| {
                ui.label(i18n.t("skills.create.title"));
                ui.text_edit_singleline(&mut edit_tmpl.title);
            });
            ui.horizontal(|ui| {
                ui.label(i18n.t("common.description"));
                ui.text_edit_singleline(&mut edit_tmpl.description);
            });
            ui.label(i18n.t("prompts.content"));
            ui.text_edit_multiline(&mut edit_tmpl.content);
            ui.horizontal(|ui| {
                ui.label(i18n.t("prompts.tags"));
                let mut tags_str = edit_tmpl.tags.join(", ");
                ui.text_edit_singleline(&mut tags_str);
                edit_tmpl.tags = tags_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            });

            let lang = i18n.lang;
            ui.horizontal(|ui| {
                if ui.button(i18n.t("prompts.save")).clicked() {
                    if let Err(err) = self.save_custom_template(&edit_tmpl) {
                        self.error = err;
                    }
                    self.edit_mode = false;
                    self.edit_template = None;
                    self.reload(lang);
                }
                if ui.button(i18n.t("prompts.delete")).clicked() {
                    if let Err(err) = self.delete_custom_template(&edit_tmpl) {
                        self.error = err;
                    }
                    self.edit_mode = false;
                    self.edit_template = None;
                    self.selected_template = None;
                    self.reload(lang);
                }
                if ui.button(i18n.t("prompts.cancel")).clicked() {
                    self.edit_mode = false;
                    self.edit_template = None;
                }
            });
        });
    }

    /// Get the base directory for prompts files.
    /// Priority:
    ///   1. Alongside the running binary (deployment)
    ///   2. Current working directory (development)
    fn prompts_base_dir() -> std::path::PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("prompts");
                if candidate.exists() {
                    return candidate;
                }
            }
        }
        std::path::PathBuf::from("prompts")
    }

    /// Save a custom template to prompts/custom/{lang}.json
    /// alongside the binary (deployment) or in CWD (development).
    fn save_custom_template(&self, tmpl: &PromptTemplate) -> Result<(), String> {
        let base_dir = Self::prompts_base_dir();
        let custom_dir = base_dir.join("custom");
        std::fs::create_dir_all(&custom_dir)
            .map_err(|e| format!("failed to create custom prompts directory: {}", e))?;

        let lang_file = match self.current_lang {
            Lang::ZhCn => "zh-CN.json",
            Lang::ZhTw => "zh-TW.json",
            Lang::En => "en.json",
        };
        let path = custom_dir.join(lang_file);

        let mut templates: Vec<PromptTemplate> = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        if let Some(pos) = templates.iter().position(|t| t.id == tmpl.id) {
            templates[pos] = tmpl.clone();
        } else {
            templates.push(tmpl.clone());
        }

        let content = serde_json::to_string_pretty(&templates)
            .map_err(|e| format!("failed to serialize templates: {}", e))?;
        std::fs::write(&path, &content)
            .map_err(|e| format!("failed to write custom prompts file: {}", e))?;
        Ok(())
    }

    /// Delete a custom template from prompts/custom/{lang}.json.
    fn delete_custom_template(&self, tmpl: &PromptTemplate) -> Result<(), String> {
        let base_dir = Self::prompts_base_dir();
        let custom_dir = base_dir.join("custom");

        let lang_file = match self.current_lang {
            Lang::ZhCn => "zh-CN.json",
            Lang::ZhTw => "zh-TW.json",
            Lang::En => "en.json",
        };
        let path = custom_dir.join(lang_file);

        if !path.exists() {
            return Ok(());
        }

        let mut templates: Vec<PromptTemplate> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default();

        templates.retain(|t| t.id != tmpl.id);

        let content = serde_json::to_string_pretty(&templates)
            .map_err(|e| format!("failed to serialize templates: {}", e))?;
        std::fs::write(&path, &content)
            .map_err(|e| format!("failed to write custom prompts file: {}", e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_with_fallback_adds_missing_templates() {
        let mut primary = vec![PromptCategory {
            id: "software_dev".to_string(),
            name: "软件开发".to_string(),
            icon: "💻".to_string(),
            templates: vec![PromptTemplate {
                id: "explain_code".to_string(),
                title: "解释代码".to_string(),
                description: String::new(),
                content: "A".to_string(),
                tags: vec![],
            }],
        }];
        let fallback = vec![PromptCategory {
            id: "software_dev".to_string(),
            name: "Software Development".to_string(),
            icon: "💻".to_string(),
            templates: vec![PromptTemplate {
                id: "architecture_design".to_string(),
                title: "Architecture Design".to_string(),
                description: String::new(),
                content: "B".to_string(),
                tags: vec![],
            }],
        }];

        PromptsView::merge_with_fallback(&mut primary, fallback);

        let cat = &primary[0];
        assert!(cat.templates.iter().any(|t| t.id == "explain_code"));
        assert!(cat.templates.iter().any(|t| t.id == "architecture_design"));
    }

    #[test]
    fn derive_command_templates_avoids_ambiguous_short_commands() {
        let mut view = PromptsView::new();
        view.collection = vec![
            PromptCategory {
                id: "business".to_string(),
                name: "Business".to_string(),
                icon: "📊".to_string(),
                templates: vec![PromptTemplate {
                    id: "risk_assessment".to_string(),
                    title: "Risk A".to_string(),
                    description: String::new(),
                    content: "A".to_string(),
                    tags: vec![],
                }],
            },
            PromptCategory {
                id: "finance".to_string(),
                name: "Finance".to_string(),
                icon: "💰".to_string(),
                templates: vec![PromptTemplate {
                    id: "risk_assessment".to_string(),
                    title: "Risk B".to_string(),
                    description: String::new(),
                    content: "B".to_string(),
                    tags: vec![],
                }],
            },
        ];

        let commands = view.derive_command_templates();
        let names: Vec<String> = commands.into_iter().map(|c| c.command).collect();

        assert!(!names.iter().any(|c| c == "/risk_assessment"));
        assert!(names.iter().any(|c| c == "/business.risk_assessment"));
        assert!(names.iter().any(|c| c == "/finance.risk_assessment"));
    }
}
