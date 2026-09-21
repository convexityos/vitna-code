use crate::workspace_fs::{read_up_to, Resolved, Workspace, MAX_FILE_BYTES};
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use std::fs;
use std::path::Path;

/// How many unreadable or partly searched paths the output lists by name.
const MAX_LISTED: usize = 20;

pub struct SearchCodeTool;

/// One search's matches, plus everything that kept it from being complete.
/// A search that could not read part of its scope must say so; reporting it
/// as "no matches" would tell the model that code is absent when it was
/// never looked at.
struct Search<'a> {
    workspace: &'a Workspace,
    query: &'a str,
    query_lower: String,
    case_sensitive: bool,
    max_results: usize,
    /// Bytes searched per file. Anything past it is reported, not read.
    file_limit: u64,
    results: Vec<String>,
    unreadable: Vec<String>,
    partly_searched: Vec<String>,
    links_skipped: usize,
    special_skipped: usize,
    limit_reached: bool,
}

impl<'a> Search<'a> {
    fn new(workspace: &'a Workspace, query: &'a str, case_sensitive: bool, max_results: usize) -> Self {
        Self {
            workspace,
            query,
            query_lower: query.to_lowercase(),
            case_sensitive,
            max_results,
            file_limit: MAX_FILE_BYTES,
            results: Vec::new(),
            unreadable: Vec::new(),
            partly_searched: Vec::new(),
            links_skipped: 0,
            special_skipped: 0,
            limit_reached: false,
        }
    }

    fn full(&mut self) -> bool {
        if self.results.len() >= self.max_results {
            self.limit_reached = true;
        }
        self.limit_reached
    }

    fn search_file(&mut self, path: &Path) {
        if self.full() {
            return;
        }
        let rel_path = self.workspace.relative(path);

        // Opened like read_file opens: a FIFO or a link swapped in during the
        // walk is refused instead of blocking or being followed.
        let mut file = match self.workspace.open_regular(path, &rel_path) {
            Ok(file) => file,
            Err(e) => {
                self.unreadable.push(e);
                return;
            }
        };
        let (raw_bytes, truncated) = match read_up_to(&mut file, self.file_limit) {
            Ok(read) => read,
            Err(e) => {
                self.unreadable.push(format!("{}: {}", rel_path, e));
                return;
            }
        };

        // Skip non-text binary files (heuristic: zero bytes in first 512 bytes)
        if raw_bytes.iter().take(512).any(|&b| b == 0) {
            return;
        }
        if truncated {
            self.partly_searched.push(rel_path.clone());
        }

        let text = String::from_utf8_lossy(&raw_bytes);
        for (idx, line) in text.lines().enumerate() {
            if self.full() {
                return;
            }

            let matches = if self.case_sensitive {
                line.contains(self.query)
            } else {
                line.to_lowercase().contains(&self.query_lower)
            };

            if matches {
                self.results.push(format!("{}:{}: {}", rel_path, idx + 1, line.trim()));
            }
        }
    }

    fn search_dir(&mut self, dir: &Path) {
        if self.full() {
            return;
        }

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                self.unreadable.push(format!("{}: {}", self.workspace.relative(dir), e));
                return;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => children.push(entry),
                Err(e) => self
                    .unreadable
                    .push(format!("{}: {}", self.workspace.relative(dir), e)),
            }
        }
        // Name order, so the same tree always yields the same results and the
        // same cut-off at max_results.
        children.sort_by_key(|entry| entry.file_name());

        for entry in children {
            if self.full() {
                return;
            }

            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str == ".git" || name_str == ".vitna" || name_str == "target" || name_str == "node_modules" {
                continue;
            }

            // The entry's own type: a link is never followed, so a link to a
            // directory outside cannot widen the search and a link cycle
            // cannot loop it.
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(e) => {
                    self.unreadable.push(format!("{}: {}", self.workspace.relative(&path), e));
                    continue;
                }
            };
            if file_type.is_symlink() {
                self.links_skipped += 1;
            } else if file_type.is_dir() {
                self.search_dir(&path);
            } else if file_type.is_file() {
                self.search_file(&path);
            } else {
                self.special_skipped += 1;
            }
        }
    }

    fn render(&self) -> String {
        let mut output = if self.unreadable.is_empty() {
            format!("Search results for '{}' ({} matches):\n", self.query, self.results.len())
        } else {
            format!(
                "Search results for '{}' ({} matches; partial, {} paths unreadable):\n",
                self.query,
                self.results.len(),
                self.unreadable.len()
            )
        };

        for res in &self.results {
            output.push_str(&format!("{}\n", res));
        }
        if self.results.is_empty() {
            output.push_str(if self.unreadable.is_empty() {
                "No matches found.\n"
            } else {
                "No matches in the paths that could be read.\n"
            });
        }

        if !self.unreadable.is_empty() {
            output.push_str(&format!(
                "Search incomplete: {} paths could not be read, so matches in them are unknown:\n",
                self.unreadable.len()
            ));
            push_listed(&mut output, &self.unreadable);
        }
        if !self.partly_searched.is_empty() {
            output.push_str(&format!(
                "Only the first {} bytes of {} larger files were searched:\n",
                self.file_limit,
                self.partly_searched.len()
            ));
            push_listed(&mut output, &self.partly_searched);
        }
        if self.limit_reached {
            output.push_str(&format!(
                "Stopped at max_results ({}); the rest of the search scope was not searched.\n",
                self.max_results
            ));
        }
        if self.links_skipped > 0 {
            output.push_str(&format!(
                "{} symbolic links were not followed.\n",
                self.links_skipped
            ));
        }
        if self.special_skipped > 0 {
            output.push_str(&format!(
                "{} special files (FIFOs, sockets, devices) were skipped.\n",
                self.special_skipped
            ));
        }
        output
    }
}

fn push_listed(output: &mut String, paths: &[String]) {
    for path in paths.iter().take(MAX_LISTED) {
        output.push_str(&format!("  {}\n", path));
    }
    if paths.len() > MAX_LISTED {
        output.push_str(&format!("  ... and {} more\n", paths.len() - MAX_LISTED));
    }
}

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_code".to_string(),
            description: "Fast workspace code search across files with line numbers and matched content.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Text substring or keyword to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional relative subpath to restrict search. Defaults to '.' (entire workspace)."
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Optional flag for case sensitivity. Defaults to false."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Optional limit on number of returned results. Defaults to 100."
                    }
                },
                "required": ["query"]
            }),
            is_mutating: false,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| "Missing required 'query' parameter".to_string())?;

        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or(".");

        let case_sensitive = args
            .get("case_sensitive")
            .and_then(|c| c.as_bool())
            .unwrap_or(false);

        let max_results = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .map(|m| m as usize)
            .unwrap_or(100);

        let workspace = Workspace::new(&ctx.workspace_root)?;
        let target = match workspace.resolve(path_str)? {
            Resolved::Existing(target) => target,
            // Searching nothing is not the same as finding nothing.
            Resolved::Missing(_) => return Err(format!("Search path does not exist: {}", path_str)),
        };

        let mut search = Search::new(&workspace, query, case_sensitive, max_results);
        if target.is_dir() {
            search.search_dir(&target);
        } else {
            search.search_file(&target);
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "search_code".to_string(),
            // A search that could not read part of its scope did not do what
            // was asked, whatever it found in the rest.
            success: search.unreadable.is_empty(),
            output: search.render(),
            preimage_hash: None,
            postimage_hash: None,
            diff: None,
            exit_code: None,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dir_link, Scratch};

    async fn search(s: &Scratch, args: serde_json::Value) -> ToolResult {
        SearchCodeTool.execute(args, &s.ctx()).await.expect("search runs")
    }

    #[tokio::test]
    async fn test_finds_matches_in_name_order() {
        let s = Scratch::new("search_basic");
        fs::write(s.ws().join("b.txt"), "needle two\n").unwrap();
        fs::write(s.ws().join("a.txt"), "x\nNeedle one\n").unwrap();
        let res = search(&s, json!({ "query": "needle" })).await;
        assert!(res.success);
        let a = res.output.find("a.txt:2: Needle one").expect("a.txt match");
        let b = res.output.find("b.txt:1: needle two").expect("b.txt match");
        assert!(a < b, "{}", res.output);
    }

    #[tokio::test]
    async fn test_links_are_not_followed() {
        let s = Scratch::new("search_links");
        fs::write(s.ws().join("a.txt"), "nothing here\n").unwrap();
        fs::write(s.outside().join("found.txt"), "needle outside\n").unwrap();
        let linked_out = dir_link(&s.outside(), &s.ws().join("out"));
        let looped = dir_link(&s.ws(), &s.ws().join("loop"));
        if !linked_out || !looped {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let res = search(&s, json!({ "query": "needle" })).await;
        assert!(!res.output.contains("needle outside"), "{}", res.output);
        assert!(res.output.contains("2 symbolic links were not followed"), "{}", res.output);
        assert!(res.output.contains("No matches found."), "{}", res.output);
    }

    #[tokio::test]
    async fn test_missing_search_path_is_an_error() {
        let s = Scratch::new("search_missing");
        let err = SearchCodeTool
            .execute(json!({ "query": "x", "path": "nope" }), &s.ctx())
            .await
            .unwrap_err();
        assert!(err.contains("does not exist"), "{}", err);
    }

    #[tokio::test]
    async fn test_result_limit_is_reported() {
        let s = Scratch::new("search_limit");
        fs::write(s.ws().join("a.txt"), "hit\nhit\nhit\n").unwrap();
        let res = search(&s, json!({ "query": "hit", "max_results": 2 })).await;
        assert!(res.output.contains("(2 matches)"), "{}", res.output);
        assert!(res.output.contains("Stopped at max_results (2)"), "{}", res.output);
    }

    #[test]
    fn test_unreadable_paths_are_never_no_matches() {
        let s = Scratch::new("search_render");
        let workspace = Workspace::new(&s.ws()).unwrap();
        let mut search = Search::new(&workspace, "needle", false, 100);
        search.unreadable.push("src/locked.rs: Permission denied".to_string());
        let out = search.render();
        assert!(!out.contains("No matches found."), "{}", out);
        assert!(out.contains("partial, 1 paths unreadable"), "{}", out);
        assert!(out.contains("src/locked.rs: Permission denied"), "{}", out);
    }

    #[test]
    fn test_large_files_are_searched_up_to_the_bound_and_reported() {
        let s = Scratch::new("search_bound");
        fs::write(s.ws().join("big.txt"), "needle early\nmore text\nneedle late\n").unwrap();
        let workspace = Workspace::new(&s.ws()).unwrap();
        let mut search = Search::new(&workspace, "needle", false, 100);
        search.file_limit = 14;
        search.search_dir(&fs::canonicalize(s.ws()).unwrap());
        assert_eq!(search.results, vec!["big.txt:1: needle early".to_string()]);
        let out = search.render();
        assert!(out.contains("Only the first 14 bytes of 1 larger files"), "{}", out);
        assert!(out.contains("  big.txt\n"), "{}", out);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_unreadable_directory_makes_the_search_partial() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new("search_unreadable");
        let locked = s.ws().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("x.txt"), "needle\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_ok() {
            // Running as root: permissions do not stop the read.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!("skipped: permissions are not enforced for this user");
            return;
        }
        let res = search(&s, json!({ "query": "needle" })).await;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!res.success);
        assert!(!res.output.contains("No matches found."), "{}", res.output);
        assert!(res.output.contains("locked"), "{}", res.output);
    }

    #[cfg(unix)]
    #[test]
    fn test_fifo_is_skipped_without_blocking() {
        let s = Scratch::new("search_fifo");
        fs::write(s.ws().join("a.txt"), "needle\n").unwrap();
        if !crate::test_support::mkfifo(&s.ws().join("pipe")) {
            eprintln!("skipped: mkfifo is unavailable");
            return;
        }
        let res = crate::test_support::execute_bounded(
            SearchCodeTool,
            json!({ "query": "needle" }),
            s.ctx(),
        )
        .expect("search runs");
        assert!(res.output.contains("a.txt:1: needle"), "{}", res.output);
        assert!(res.output.contains("1 special files"), "{}", res.output);
    }
}
