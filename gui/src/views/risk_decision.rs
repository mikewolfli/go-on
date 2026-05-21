use crate::i18n::I18n;

#[derive(Debug, Clone, Default)]
pub struct RiskDecisionDraft {
    pub is_high: bool,
    pub review_required: bool,
    pub strategy: String,
    pub reasons: String,
}

pub struct RiskDecisionView {
    is_high: bool,
    review_required: bool,
    strategy: String,
    reasons: String,
}

impl RiskDecisionView {
    pub fn new() -> Self {
        Self {
            is_high: false,
            review_required: false,
            strategy: String::new(),
            reasons: String::new(),
        }
    }

    fn build_block(&self, i18n: &I18n) -> String {
        let state = if self.is_high {
            i18n.t("chat.riskDecisionHigh")
        } else {
            i18n.t("chat.riskDecisionNormal")
        };
        let review = if self.review_required {
            i18n.t("chat.riskDecisionReviewRequired")
        } else {
            i18n.t("chat.riskDecisionNoReview")
        };
        format!(
            "[{}]\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}",
            i18n.t("chat.riskDecisionTitle"),
            i18n.t("chat.riskDecisionState"),
            state,
            i18n.t("chat.riskDecisionReview"),
            review,
            i18n.t("chat.riskDecisionStrategy"),
            self.strategy.trim(),
            i18n.t("chat.riskDecisionReasons"),
            self.reasons.trim()
        )
    }

    pub fn draft(&self) -> RiskDecisionDraft {
        RiskDecisionDraft {
            is_high: self.is_high,
            review_required: self.review_required,
            strategy: self.strategy.clone(),
            reasons: self.reasons.clone(),
        }
    }

    pub fn apply_draft(&mut self, draft: &RiskDecisionDraft) {
        self.is_high = draft.is_high;
        self.review_required = draft.review_required;
        self.strategy = draft.strategy.clone();
        self.reasons = draft.reasons.clone();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, i18n: &I18n) -> Option<String> {
        let mut inserted = None;
        ui.heading(i18n.t("chat.riskDecisionTitle"));
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(i18n.t("chat.riskDecisionState"));
            ui.selectable_value(&mut self.is_high, false, i18n.t("chat.riskDecisionNormal"));
            ui.selectable_value(&mut self.is_high, true, i18n.t("chat.riskDecisionHigh"));
        });

        ui.horizontal(|ui| {
            ui.label(i18n.t("chat.riskDecisionReview"));
            ui.selectable_value(
                &mut self.review_required,
                true,
                i18n.t("chat.riskDecisionReviewRequired"),
            );
            ui.selectable_value(
                &mut self.review_required,
                false,
                i18n.t("chat.riskDecisionNoReview"),
            );
        });

        ui.add_space(8.0);
        ui.label(i18n.t("chat.riskDecisionStrategy"));
        ui.add(egui::TextEdit::singleline(&mut self.strategy).desired_width(f32::INFINITY));

        ui.label(i18n.t("chat.riskDecisionReasons"));
        ui.add(
            egui::TextEdit::multiline(&mut self.reasons)
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );

        ui.add_space(8.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(self.build_block(i18n));
        });

        ui.add_space(8.0);
        if ui.button(i18n.t("chat.templateInsert")).clicked() {
            inserted = Some(self.build_block(i18n));
        }

        inserted
    }
}
