use crate::views::{
    about::AboutView, autotune::AutoTuneView, chat::ChatView, config_editor::ConfigEditorView,
    monitor::MonitorView, prompts::PromptsView, providers::ProvidersView,
    risk_decision::RiskDecisionView, security::SecurityView, setup::SetupView, skills::SkillsView,
    workflow::WorkflowView,
};

/// Holds all view structs used by the application.
/// This reduces the field count on GoOnApp by bundling related view state.
pub struct ViewRegistry {
    pub setup_view: SetupView,
    pub monitor_view: MonitorView,
    pub chat_view: ChatView,
    pub skills_view: SkillsView,
    pub workflow_view: WorkflowView,
    pub autotune_view: AutoTuneView,
    pub security_view: SecurityView,
    pub config_editor_view: ConfigEditorView,
    pub prompts_view: PromptsView,
    pub risk_decision_view: RiskDecisionView,
    pub providers_view: ProvidersView,
    pub about_view: AboutView,
}

impl ViewRegistry {
    pub fn new() -> Self {
        Self {
            setup_view: SetupView::new(),
            monitor_view: MonitorView::new(),
            chat_view: ChatView::new(),
            skills_view: SkillsView::new(),
            workflow_view: WorkflowView::new(),
            autotune_view: AutoTuneView::new(),
            security_view: SecurityView::new(),
            config_editor_view: ConfigEditorView::new(),
            prompts_view: PromptsView::new(),
            risk_decision_view: RiskDecisionView::new(),
            providers_view: ProvidersView::new(),
            about_view: AboutView::new(),
        }
    }
}
