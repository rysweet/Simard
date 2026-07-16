//! Optional kitchen prep app generation.
//!
//! Emits a single self-contained `prep_app.html` — a runnable, offline kitchen
//! prep checklist — from the prep schedule. It is the Gastronome analogue of
//! Atelier's optional CAD exports: no external tool is required, so the app is
//! always producible, but it is only written when the operator asks for it
//! (`--prep-app`). The generated file embeds the schedule as JSON and needs no
//! network access or build step to run.

use std::path::Path;

use serde_json::json;

use super::error::{GastronomeError, GastronomeResult};
use super::menu::Menu;
use super::schedule::PrepSchedule;

/// Outcome of an optional prep-app generation attempt.
#[derive(Debug, Clone)]
pub struct AppReport {
    pub produced: bool,
    pub detail: String,
}

/// Render the prep schedule into a self-contained HTML kitchen app string.
pub fn render_prep_app(menu: &Menu, schedule: &PrepSchedule) -> String {
    let tasks: Vec<_> = schedule
        .tasks
        .iter()
        .map(|t| {
            json!({
                "order": t.order,
                "dish": t.dish,
                "task": t.task,
                "station": t.station,
                "minutes": t.minutes,
                "start_offset_min": t.start_offset_min,
                "start_clock": t.start_clock,
            })
        })
        .collect();

    let payload = json!({
        "event": menu.event,
        "guests": menu.guests,
        "service_time": schedule.service_time,
        "total_minutes": schedule.total_minutes,
        "tasks": tasks,
    });

    // `serde_json` output is HTML-safe for a <script type="application/json">
    // block: it escapes nothing dangerous except we still guard against a
    // literal "</" sequence that could close the script tag early.
    let data = payload.to_string().replace("</", "<\\/");
    let event = html_escape(&menu.event);

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Prep — {event}</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font-family: system-ui, sans-serif; margin: 0; padding: 1rem; max-width: 46rem; }}
  h1 {{ font-size: 1.4rem; margin: 0 0 .25rem; }}
  .meta {{ color: #666; margin-bottom: 1rem; }}
  ol {{ list-style: none; padding: 0; }}
  li {{ display: flex; gap: .6rem; align-items: flex-start; padding: .5rem .25rem;
        border-bottom: 1px solid #ccc3; }}
  li.done label {{ text-decoration: line-through; opacity: .55; }}
  .when {{ font-variant-numeric: tabular-nums; color: #888; min-width: 4.5rem; }}
  .station {{ font-size: .8rem; background: #8883; border-radius: .5rem; padding: 0 .4rem; }}
  .bar {{ position: sticky; top: 0; background: Canvas; padding: .5rem 0; }}
  progress {{ width: 100%; height: 1rem; }}
</style>
</head>
<body>
<h1>{event}</h1>
<div class="meta" id="meta"></div>
<div class="bar"><progress id="pg" value="0" max="1"></progress>
  <div id="count"></div></div>
<ol id="list"></ol>
<script type="application/json" id="data">{data}</script>
<script>
(function () {{
  var plan = JSON.parse(document.getElementById("data").textContent);
  var meta = document.getElementById("meta");
  meta.textContent = "Guests: " + plan.guests +
    (plan.service_time ? " · Service " + plan.service_time : "") +
    " · Prep starts " + Math.round(plan.total_minutes) + " min before service";
  var list = document.getElementById("list");
  var pg = document.getElementById("pg");
  var count = document.getElementById("count");
  pg.max = Math.max(plan.tasks.length, 1);
  function refresh() {{
    var done = document.querySelectorAll("li.done").length;
    pg.value = done;
    count.textContent = done + " / " + plan.tasks.length + " tasks done";
  }}
  plan.tasks.forEach(function (t) {{
    var li = document.createElement("li");
    var cb = document.createElement("input");
    cb.type = "checkbox";
    cb.id = "t" + t.order;
    cb.addEventListener("change", function () {{
      li.classList.toggle("done", cb.checked);
      refresh();
    }});
    var when = document.createElement("span");
    when.className = "when";
    when.textContent = t.start_clock ? t.start_clock : "T-" + Math.round(t.start_offset_min);
    var label = document.createElement("label");
    label.htmlFor = cb.id;
    var station = document.createElement("span");
    station.className = "station";
    station.textContent = t.station;
    label.append(station, document.createTextNode(" " + t.task + " — " + t.dish +
      " (" + Math.round(t.minutes) + " min)"));
    li.append(cb, when, label);
    list.append(li);
  }});
  refresh();
}})();
</script>
</body>
</html>
"#
    )
}

/// Write the prep app to `path`. Never fails the whole package: on a write
/// error it returns a not-produced report with the reason.
pub fn write_prep_app(
    menu: &Menu,
    schedule: &PrepSchedule,
    path: &Path,
) -> GastronomeResult<AppReport> {
    let html = render_prep_app(menu, schedule);
    match std::fs::write(path, &html) {
        Ok(()) => Ok(AppReport {
            produced: true,
            detail: format!("{} prep task(s)", schedule.tasks.len()),
        }),
        Err(e) => Err(GastronomeError::io(
            format!("writing prep app {}", path.display()),
            e,
        )),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::brief::MenuBrief;
    use crate::gastronome::menu::scale;
    use crate::gastronome::schedule::build_schedule;

    const BRIEF: &str = r#"{
        "event":"Bistro <night>","guests":6,"service_time":"18:30",
        "dishes":[{"name":"Soup","course":"starter",
            "ingredients":[{"name":"Squash","qty_per_serving":100,"unit":"g"}],
            "prep":[{"task":"Roast","minutes":30,"station":"oven"}]}]}"#;

    fn parts() -> (Menu, PrepSchedule) {
        let brief = MenuBrief::from_json_bytes(BRIEF.as_bytes()).unwrap();
        (scale(&brief), build_schedule(&brief))
    }

    #[test]
    fn app_is_self_contained_html_with_embedded_data() {
        let (m, s) = parts();
        let html = render_prep_app(&m, &s);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("application/json"));
        assert!(html.contains("Roast"));
        // No external resources.
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("src="));
    }

    #[test]
    fn event_name_is_html_escaped_in_title() {
        let (m, s) = parts();
        let html = render_prep_app(&m, &s);
        assert!(html.contains("Bistro &lt;night&gt;"));
    }

    #[test]
    fn write_prep_app_produces_file() {
        let (m, s) = parts();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prep_app.html");
        let report = write_prep_app(&m, &s, &path).unwrap();
        assert!(report.produced);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
}
