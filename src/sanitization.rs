pub fn objective_metadata(objective: &str) -> String {
    let chars = objective.chars().count();
    let words = objective.split_whitespace().count();
    let lines = if objective.is_empty() {
        0
    } else {
        objective.lines().count()
    };

    format!("objective-metadata(chars={chars}, words={words}, lines={lines})")
}
