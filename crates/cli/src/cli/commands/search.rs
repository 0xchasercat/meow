//! `meow search` — query the npm registry for packages (the cargo/npm `search`
//! analogue). Every byte of output flows through the `meow-ui` facade so it stays
//! on-brand in a TTY (paw spinner, branded table) and degrades cleanly — plain,
//! no animation, no color — in pipes, CI, and other non-interactive environments.

use std::process::ExitCode;

use meow_ui::table::Align;

use crate::cli::{hiss, ui, SearchArgs};

const NPM_SEARCH_URL: &str = "https://registry.npmjs.org/-/v1/search";
const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 250;

pub fn cmd_search(args: &SearchArgs) -> ExitCode {
    let joined = args.query.join(" ");
    let query = joined.trim();
    if query.is_empty() {
        hiss("meow search: provide a search term, e.g. `meow search vite`");
        return ExitCode::FAILURE;
    }
    let limit = args
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            hiss(&format!("meow search: cannot start async runtime: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let u = ui();
    // Inert in non-interactive environments (no thread, no control chars).
    let spinner = u.spinner(format!("searching the registry for \"{query}\""));
    let result = runtime.block_on(fetch_search(query, limit));
    spinner.clear();

    let objects = match result {
        Ok(objects) => objects,
        Err(err) => {
            hiss(&format!("meow search: {err}"));
            return ExitCode::FAILURE;
        }
    };

    if args.json {
        match serde_json::to_string_pretty(&objects) {
            Ok(text) => u.out(&text),
            Err(err) => {
                hiss(&format!("meow search: cannot serialize results: {err}"));
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    if objects.is_empty() {
        u.info(&format!("no packages found for \"{query}\""));
        return ExitCode::SUCCESS;
    }

    let rows = search_rows(&objects);
    let count = rows.len();
    u.table(
        &["NAME", "VERSION", "DESCRIPTION"],
        &rows,
        &[Align::Left, Align::Right, Align::Left],
    );
    u.note(&format!(
        "{count} result(s) · run `meow add <name>` to install"
    ));
    ExitCode::SUCCESS
}

/// Map the registry's `objects` array into `[name, version, description]` table
/// rows, tolerating missing fields (the registry omits `description` freely).
fn search_rows(objects: &[serde_json::Value]) -> Vec<Vec<String>> {
    objects
        .iter()
        .map(|object| {
            let package = object.get("package");
            let field = |key: &str| {
                package
                    .and_then(|p| p.get(key))
                    .and_then(|value| value.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            let mut name = field("name");
            if name.is_empty() {
                name = "<unknown>".to_owned();
            }
            let description = field("description").replace(['\n', '\r'], " ");
            vec![name, field("version"), description]
        })
        .collect()
}

/// Query the npm registry search endpoint and return the raw `objects` array.
async fn fetch_search(query: &str, limit: usize) -> Result<Vec<serde_json::Value>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("meow/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| format!("cannot build HTTP client: {err}"))?;

    let size = limit.to_string();
    let response = client
        .get(NPM_SEARCH_URL)
        .query(&[("text", query), ("size", size.as_str())])
        .send()
        .await
        .map_err(|err| format!("registry request failed: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("registry returned HTTP {}", status.as_u16()));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("cannot read registry response: {err}"))?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("cannot parse registry response: {err}"))?;

    Ok(body
        .get("objects")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::search_rows;
    use serde_json::json;

    #[test]
    fn maps_registry_objects_to_rows() {
        let objects = vec![
            json!({"package": {
                "name": "vite",
                "version": "8.1.2",
                "description": "Native-ESM dev server and build tool",
            }}),
            // The registry frequently omits `description`.
            json!({"package": { "name": "vitest", "version": "2.0.0" }}),
            // A malformed object with no `package` key falls back gracefully.
            json!({ "score": { "final": 0.1 } }),
        ];
        let rows = search_rows(&objects);
        assert_eq!(
            rows[0],
            vec!["vite", "8.1.2", "Native-ESM dev server and build tool"]
        );
        assert_eq!(rows[1], vec!["vitest", "2.0.0", ""]);
        assert_eq!(rows[2], vec!["<unknown>", "", ""]);
    }

    #[test]
    fn flattens_multiline_descriptions() {
        let objects = vec![json!({"package": {
            "name": "x",
            "version": "1.0.0",
            "description": "line1\nline2\rline3",
        }})];
        let rows = search_rows(&objects);
        assert_eq!(rows[0][2], "line1 line2 line3");
    }
}
