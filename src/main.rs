use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer};
use clap::Parser;
use notify::{RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use pulldown_cmark::{html, Options, Parser as MdParser};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "jazz", about = "Serve rendered markdown files over HTTP")]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value_t = 7705)]
    port: u16,

    /// Address to bind to
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: String,
}

/// Skip these directory names during crawl
const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "vendor", "build", "dist", "cache",
    "Cache", "Caches", "Library", "Applications", "Pictures", "Music",
    "Movies", "Photos", "Downloads", "__pycache__", "venv", ".venv",
    "Trash", ".Trash", "Containers", "WebKit", "Logs",
    "Documents", "Desktop", "Public", "openclaw", "go",
];

struct MdIndex {
    /// Set of canonical directory paths that contain .md files (directly or in children)
    dirs_with_md: HashSet<PathBuf>,
}

impl MdIndex {
    fn build(home: &Path) -> Self {
        let mut dirs = HashSet::new();
        Self::crawl(home, &mut dirs, 6);
        MdIndex { dirs_with_md: dirs }
    }

    /// Returns true if this dir or any child has .md files
    fn crawl(dir: &Path, result: &mut HashSet<PathBuf>, depth: u8) -> bool {
        if depth == 0 {
            return false;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return false,
        };
        let mut found = false;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if let Ok(ft) = entry.file_type() {
                // Skip symlinks to avoid loops
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_file() && name.ends_with(".md") {
                    found = true;
                }
                if ft.is_dir() {
                    if Self::crawl(&entry.path(), result, depth - 1) {
                        found = true;
                    }
                }
            }
        }
        if found {
            if let Ok(canonical) = dir.canonicalize() {
                result.insert(canonical);
            } else {
                result.insert(dir.to_path_buf());
            }
        }
        found
    }

    fn contains_dir(&self, dir: &Path) -> bool {
        self.dirs_with_md.contains(dir)
    }
}

struct AppState {
    home_dir: PathBuf,
    index: Arc<RwLock<MdIndex>>,
}

impl AppState {
    fn get_index(&self) -> std::sync::RwLockReadGuard<'_, MdIndex> {
        self.index.read().unwrap()
    }
}

const CSS: &str = r#"
:root {
    --bg: #1a1a2e;
    --surface: #16213e;
    --text: #e0e0e0;
    --text-muted: #8a8a9a;
    --accent: #e94560;
    --link: #64b5f6;
    --code-bg: #0f3460;
    --border: #2a2a4a;
}
* { margin: 0; padding: 0; box-sizing: border-box; }
body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: var(--bg);
    color: var(--text);
    line-height: 1.7;
    max-width: 860px;
    margin: 0 auto;
    padding: 2rem 1.5rem;
}
a { color: var(--link); text-decoration: none; }
a:hover { text-decoration: underline; }
h1, h2, h3, h4, h5, h6 {
    color: #fff;
    margin: 1.5em 0 0.5em;
    line-height: 1.3;
}
h1 { font-size: 2em; border-bottom: 2px solid var(--accent); padding-bottom: 0.3em; }
h2 { font-size: 1.5em; border-bottom: 1px solid var(--border); padding-bottom: 0.2em; }
p { margin: 0.8em 0; }
code {
    background: var(--code-bg);
    padding: 0.15em 0.4em;
    border-radius: 4px;
    font-size: 0.9em;
}
pre {
    background: var(--code-bg);
    padding: 1em;
    border-radius: 8px;
    overflow-x: auto;
    margin: 1em 0;
}
pre code { background: none; padding: 0; }
blockquote {
    border-left: 4px solid var(--accent);
    padding-left: 1em;
    color: var(--text-muted);
    margin: 1em 0;
}
ul, ol { padding-left: 1.5em; margin: 0.5em 0; }
li { margin: 0.3em 0; }
table { border-collapse: collapse; width: 100%; margin: 1em 0; }
th, td { border: 1px solid var(--border); padding: 0.5em 0.8em; text-align: left; }
th { background: var(--surface); color: #fff; }
img { max-width: 100%; height: auto; border-radius: 8px; }
hr { border: none; border-top: 1px solid var(--border); margin: 2em 0; }
.breadcrumb { color: var(--text-muted); margin-bottom: 1.5em; font-size: 0.9em; }
.breadcrumb a { color: var(--link); }
.breadcrumb span { margin: 0 0.3em; }
.dir-listing { list-style: none; padding: 0; }
.dir-listing li { padding: 0.4em 0; border-bottom: 1px solid var(--border); }
.dir-listing li:last-child { border-bottom: none; }
.dir-listing .icon { margin-right: 0.5em; }
@media (max-width: 600px) {
    body { padding: 1rem; }
    h1 { font-size: 1.5em; }
}
"#;

fn render_markdown(content: &str) -> String {
    let opts = Options::all();
    let parser = MdParser::new_ext(content, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

fn breadcrumb_html(path: &str) -> String {
    let mut crumbs = vec![format!(r#"<a href="/">~</a>"#)];
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        let href = format!("/{}", parts[..=i].join("/"));
        if i == parts.len() - 1 {
            crumbs.push(format!("<strong>{}</strong>", part));
        } else {
            crumbs.push(format!(r#"<a href="{}">{}</a>"#, href, part));
        }
    }
    format!(r#"<div class="breadcrumb">{}</div>"#, crumbs.join(r#"<span>/</span>"#))
}

fn html_page(title: &str, breadcrumb: &str, body: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} — Jazz</title>
<style>{CSS}</style>
</head>
<body>
{breadcrumb}
{body}
</body>
</html>"#
    )
}

fn dir_listing(dir: &Path, url_path: &str, index: &MdIndex) -> std::io::Result<String> {
    let mut entries: Vec<(bool, String, String)> = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type()?.is_dir();
        let trailing = if is_dir { "/" } else { "" };
        let href = if url_path.is_empty() || url_path == "/" {
            format!("/{}{}", name, trailing)
        } else {
            format!("{}/{}{}", url_path.trim_end_matches('/'), name, trailing)
        };
        if is_dir {
            // Only show dirs the index knows have markdown
            if let Ok(canonical) = entry.path().canonicalize() {
                if index.contains_dir(&canonical) {
                    entries.push((true, name, href));
                }
            }
        } else if name.ends_with(".md") {
            entries.push((false, name, href));
        }
    }

    entries.sort_by(|a, b| {
        b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });

    let items: Vec<String> = entries
        .iter()
        .map(|(is_dir, name, href)| {
            let icon = if *is_dir { "📁" } else { "📄" };
            format!(r#"<li><span class="icon">{icon}</span><a href="{href}">{name}</a></li>"#)
        })
        .collect();

    Ok(format!(r#"<ul class="dir-listing">{}</ul>"#, items.join("\n")))
}

async fn serve_path(req: HttpRequest, data: web::Data<AppState>) -> HttpResponse {
    let req_path = req.path();
    let clean_path = req_path.trim_start_matches('/');

    let fs_path = if clean_path.is_empty() {
        data.home_dir.clone()
    } else {
        data.home_dir.join(clean_path)
    };

    let canonical = match fs_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return HttpResponse::NotFound().body(
            html_page("Not Found", &breadcrumb_html(clean_path), "<h1>404 — Not Found</h1>")
        ),
    };

    if !canonical.starts_with(&data.home_dir) {
        return HttpResponse::Forbidden().body(
            html_page("Forbidden", "", "<h1>403 — Forbidden</h1><p>Access restricted to user home directory.</p>")
        );
    }

    if canonical.is_dir() {
        let index = data.get_index();
        let readme = canonical.join("README.md");
        if readme.exists() {
            let content = std::fs::read_to_string(&readme).unwrap_or_default();
            let rendered = render_markdown(&content);
            let breadcrumb = breadcrumb_html(clean_path);
            let listing = dir_listing(&canonical, req_path, &index).unwrap_or_default();
            let body = format!(
                "{rendered}<hr><h3>Files in this directory</h3>{listing}"
            );
            return HttpResponse::Ok()
                .content_type("text/html; charset=utf-8")
                .body(html_page("README.md", &breadcrumb, &body));
        }

        let breadcrumb = breadcrumb_html(clean_path);
        let listing = dir_listing(&canonical, req_path, &index).unwrap_or_default();
        let title = if clean_path.is_empty() { "~" } else { clean_path };
        return HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(html_page(title, &breadcrumb, &listing));
    }

    if canonical.extension().map(|e| e == "md").unwrap_or(false) {
        let content = match std::fs::read_to_string(&canonical) {
            Ok(c) => c,
            Err(_) => return HttpResponse::InternalServerError().body("Failed to read file"),
        };
        let rendered = render_markdown(&content);
        let breadcrumb = breadcrumb_html(clean_path);
        return HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(html_page(clean_path, &breadcrumb, &rendered));
    }

    // Serve static assets (images, etc.) so markdown image refs work
    if let Some(ext) = canonical.extension().and_then(|e| e.to_str()) {
        let mime = match ext.to_lowercase().as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            "svg" => Some("image/svg+xml"),
            "ico" => Some("image/x-icon"),
            "pdf" => Some("application/pdf"),
            _ => None,
        };
        if let Some(content_type) = mime {
            if let Ok(bytes) = std::fs::read(&canonical) {
                return HttpResponse::Ok()
                    .content_type(content_type)
                    .body(bytes);
            }
        }
    }

    HttpResponse::NotFound().body(
        html_page("Not Found", &breadcrumb_html(clean_path), "<h1>404 — Only markdown and image files are served</h1>")
    )
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let home_dir = dirs::home_dir().expect("Could not determine home directory");
    println!("🎷 Jazz starting on http://{}:{}", args.bind, args.port);
    println!("   Serving markdown from: {}", home_dir.display());
    // Start with empty index, build async
    let index = Arc::new(RwLock::new(MdIndex {
        dirs_with_md: HashSet::new(),
    }));

    let state = web::Data::new(AppState {
        home_dir: home_dir.clone(),
        index: index.clone(),
    });

    // Build index + start watcher + periodic poll in background
    let index_bg = index.clone();
    let home_bg = home_dir.clone();
    std::thread::spawn(move || {
        // Initial index build
        println!("   Building index...");
        let built = MdIndex::build(&home_bg);
        let dir_count = built.dirs_with_md.len();
        let watched_dirs: Vec<PathBuf> = built.dirs_with_md.iter().cloned().collect();
        {
            let mut idx = index_bg.write().unwrap();
            *idx = built;
        }
        println!("   Indexed {} directories with markdown", dir_count);

        // Set up fsnotify watcher on indexed directories
        let index_watch = index_bg.clone();
        let home_watch = home_bg.clone();
        let debounce = Arc::new(RwLock::new(Instant::now()));
        let debounce_w = debounce.clone();

        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let dominated = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                );
                let is_md = event.paths.iter().any(|p| {
                    p.extension().map(|e| e == "md").unwrap_or(false)
                        || p.is_dir()
                });
                if dominated && is_md {
                    // Debounce: rebuild at most once per 2 seconds
                    let last = *debounce_w.read().unwrap();
                    if last.elapsed() < Duration::from_secs(2) {
                        return;
                    }
                    *debounce_w.write().unwrap() = Instant::now();
                    let new_index = MdIndex::build(&home_watch);
                    let mut idx = index_watch.write().unwrap();
                    *idx = new_index;
                    println!("   Index refreshed (fsnotify): {} directories", idx.dirs_with_md.len());
                }
            }
        }).expect("Failed to create file watcher");

        // Watch all indexed directories (non-recursive — they're already leaf dirs)
        for dir in &watched_dirs {
            let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
        }
        // Also watch home dir for new top-level additions
        let _ = watcher.watch(&home_bg, RecursiveMode::NonRecursive);
        println!("   Watching {} directories for changes", watched_dirs.len() + 1);

        // Periodic full re-index every 10 minutes to catch new top-level dirs
        loop {
            std::thread::sleep(Duration::from_secs(600));
            let new_index = MdIndex::build(&home_bg);
            let new_dirs: Vec<PathBuf> = new_index.dirs_with_md.iter().cloned().collect();
            {
                let mut idx = index_bg.write().unwrap();
                *idx = new_index;
            }
            // Update watcher with any new dirs
            for dir in &new_dirs {
                let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
            }
            println!("   Index refreshed (periodic): {} directories", new_dirs.len());
        }
    });

    HttpServer::new(move || {
        App::new()
            .app_data(state.clone())
            .default_service(web::to(serve_path))
    })
    .bind((args.bind.as_str(), args.port))?
    .run()
    .await
}
