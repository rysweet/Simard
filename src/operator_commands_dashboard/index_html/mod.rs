mod part_00;
mod part_01;
mod part_02;
mod part_03;
mod part_04;
mod part_05;

use part_00::PART_00;
use part_01::PART_01;
use part_02::PART_02;
use part_03::PART_03;
use part_04::PART_04;
use part_05::PART_05;

/// Concatenated dashboard HTML/JS, assembled from per-segment string consts
/// so that no single Rust source file exceeds the 400 LOC cap (#1266).
pub(crate) fn index_html_string() -> String {
    format!("{PART_00} {PART_01} {PART_02} {PART_03} {PART_04} {PART_05}")
}

#[cfg(test)]
pub(crate) static INDEX_HTML: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(index_html_string);
