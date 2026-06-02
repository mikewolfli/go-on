pub(crate) fn push_metric_header(
    lines: &mut Vec<String>,
    name: &str,
    metric_type: &str,
    help: &str,
) {
    lines.push(format!("# HELP {} {}", name, help));
    lines.push(format!("# TYPE {} {}", name, metric_type));
}

#[allow(dead_code)]
pub(crate) fn push_scalar_metric(
    lines: &mut Vec<String>,
    name: &str,
    metric_type: &str,
    help: &str,
    value: impl std::fmt::Display,
) {
    push_metric_header(lines, name, metric_type, help);
    lines.push(format!("{} {}", name, value));
}
