use futures::future::{AbortHandle, Abortable};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;

struct GenState(Mutex<Option<AbortHandle>>);

const DEFAULT_MODEL: &str = "gemma3:4b";
const OLLAMA_BASE: &str = "http://localhost:11434";

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
struct Settings {
    ollama_model: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct Question {
    question: String,
    answers: Vec<String>,
    correct_index: usize,
    explanations: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Attempt {
    correct: usize,
    total: usize,
    at: u64,
}

#[derive(Serialize, Deserialize, Clone)]
struct QuestionSet {
    name: String,
    exam: String,
    providers: Vec<String>,
    created_at: u64,
    questions: Vec<Question>,
    #[serde(default)]
    attempts: Vec<Attempt>,
}

#[derive(Serialize)]
struct SetSummary {
    name: String,
    exam: String,
    created_at: u64,
    question_count: usize,
    attempts: Vec<Attempt>,
    providers: Vec<String>,
}

fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?;
    fs::create_dir_all(dir.join("sets")).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------- settings ----------

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Result<Settings, String> {
    let path = data_dir(&app)?.join("settings.json");
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    // tolerate a UTF-8 BOM and corrupt files: fall back to defaults instead of failing
    let raw = raw.trim_start_matches('\u{feff}');
    Ok(serde_json::from_str(raw).unwrap_or_default())
}

#[tauri::command]
fn save_settings(app: tauri::AppHandle, settings: Settings) -> Result<(), String> {
    let path = data_dir(&app)?.join("settings.json");
    let raw = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

// ---------- level / XP progress ----------

#[derive(Serialize, Deserialize, Clone)]
struct Progress {
    xp: u64,
    level: u32,
}

#[tauri::command]
fn get_progress(app: tauri::AppHandle) -> Result<Progress, String> {
    let path = data_dir(&app)?.join("progress.json");
    if !path.exists() {
        return Ok(Progress { xp: 0, level: 1 });
    }
    let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let raw = raw.trim_start_matches('\u{feff}');
    Ok(serde_json::from_str(raw).unwrap_or(Progress { xp: 0, level: 1 }))
}

#[tauri::command]
fn save_progress(app: tauri::AppHandle, progress: Progress) -> Result<(), String> {
    let path = data_dir(&app)?.join("progress.json");
    let raw = serde_json::to_string_pretty(&progress).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

fn default_model(settings: &Settings) -> String {
    let m = settings.ollama_model.trim();
    if m.is_empty() || m.contains(' ') {
        DEFAULT_MODEL.to_string()
    } else {
        m.to_string()
    }
}

// ---------- question sets ----------

#[tauri::command]
fn list_sets(app: tauri::AppHandle) -> Result<Vec<SetSummary>, String> {
    let dir = data_dir(&app)?.join("sets");
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(entry.path()).map_err(|e| e.to_string())?;
        if let Ok(set) = serde_json::from_str::<QuestionSet>(&raw) {
            out.push(SetSummary {
                name: set.name,
                exam: set.exam,
                created_at: set.created_at,
                question_count: set.questions.len(),
                attempts: set.attempts,
                providers: set.providers,
            });
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

#[tauri::command]
fn load_set(app: tauri::AppHandle, name: String) -> Result<QuestionSet, String> {
    let path = data_dir(&app)?
        .join("sets")
        .join(format!("{}.json", sanitize_name(&name)));
    let raw = fs::read_to_string(path).map_err(|e| format!("cannot open set: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_set(app: tauri::AppHandle, name: String) -> Result<(), String> {
    let path = data_dir(&app)?
        .join("sets")
        .join(format!("{}.json", sanitize_name(&name)));
    fs::remove_file(path).map_err(|e| e.to_string())
}

fn save_set(app: &tauri::AppHandle, set: &QuestionSet) -> Result<(), String> {
    let path = data_dir(app)?
        .join("sets")
        .join(format!("{}.json", sanitize_name(&set.name)));
    let raw = serde_json::to_string_pretty(set).map_err(|e| e.to_string())?;
    fs::write(path, raw).map_err(|e| e.to_string())
}

// ---------- prompt + parsing ----------

fn difficulty_clause(difficulty: &str) -> (String, String) {
    let guidance = match difficulty {
        "easy" => "Keep every question at EASY difficulty: fundamental definitions, \
single-concept recall, and terminology — the kind of question that checks basic \
familiarity with a topic.",
        "medium" => "Keep every question at MEDIUM difficulty: applied, scenario-based \
questions that require combining two or three related concepts, similar to the bulk of \
questions on a real certification exam.",
        "hard" => "Keep every question at HARD difficulty: complex, multi-step scenarios \
that require analysis, trade-off judgment, or knowledge of edge cases — comparable to the \
hardest questions on the real exam.",
        _ => "Vary the difficulty naturally across the set: include some fundamental \
recall questions, mostly applied scenario questions, and a few genuinely hard ones — the \
same spread a real certification exam would have.",
    };
    let label = if difficulty.is_empty() {
        String::new()
    } else {
        format!(" at {difficulty} difficulty")
    };
    (label, guidance.to_string())
}

fn build_prompt(exam: &str, count: usize, difficulty: &str) -> String {
    let (label, guidance) = difficulty_clause(difficulty);
    format!(
        "You are an expert certification-exam item writer with deep subject-matter expertise \
in \"{exam}\". Generate exactly {count} multiple-choice practice questions for this exam{label}.\n\n\
Guidelines:\n\
- Cover a broad, representative range of the exam's official objectives; do not repeat the \
same narrow sub-topic more than twice.\n\
- Write questions the way they appear on real certification exams: clear and unambiguous, \
with scenario-based phrasing where that fits the topic. Exactly one answer must be correct.\n\
- All 4 answer options must be plausible and similar in length and style. Wrong answers \
should reflect realistic misconceptions or common mistakes, never be obviously silly or \
off-topic.\n\
- Vary which position (0-3) holds the correct answer across questions so the pattern isn't \
predictable — do not favor any one letter.\n\
- Never repeat the same question wording or scenario twice in the set.\n\
- Do NOT prefix any answer or explanation with its own letter or number \
(no \"A.\", \"B)\", \"C:\", \"3.\", etc.) — the app already displays that label \
automatically. Write only the content itself.\n\
- {guidance}\n\n\
Respond with ONLY a JSON array, no markdown fences, no commentary, no text before or after \
the array. Each element must be an object with exactly these fields:\n\
  \"question\": string — the question text\n\
  \"answers\": array of exactly 4 answer strings\n\
  \"correct_index\": integer 0-3 — index of the correct answer\n\
  \"explanations\": array of exactly 4 strings — for each answer, explain concisely why it \
is correct or incorrect, teaching the underlying concept so the learner improves even from a \
wrong guess\n\n\
Each explanation should be 1-3 sentences, specific and educational — avoid generic filler \
like \"this is correct because it is the best option.\""
    )
}

/// Find the first array of question-shaped objects inside an arbitrary JSON
/// value — models in grammar-constrained JSON mode sometimes wrap the array
/// in an object (e.g. `{"questions": [...]}`) instead of returning it bare.
fn find_question_array(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let serde_json::Value::Array(arr) = value {
        if !arr.is_empty() {
            return Some(arr);
        }
    }
    if let serde_json::Value::Object(map) = value {
        for v in map.values() {
            if let Some(found) = find_question_array(v) {
                return Some(found);
            }
        }
    }
    None
}

fn extract_json_array(text: &str) -> Result<Vec<Question>, String> {
    // Local models sometimes emit raw control characters (e.g. real newlines)
    // inside JSON strings, which strict parsers reject. Try the raw text
    // first, then a version with control characters blanked out.
    let cleaned: String = text
        .chars()
        .map(|c| if (c as u32) < 0x20 && c != ' ' { ' ' } else { c })
        .collect();

    let parsed: serde_json::Value = serde_json::from_str(text)
        .or_else(|_| serde_json::from_str(&cleaned))
        .or_else(|_| {
            // fall back to slicing out the outermost [...] or {...} span
            let candidates = [('[', ']'), ('{', '}')];
            for (open, close) in candidates {
                if let (Some(start), Some(end)) = (cleaned.find(open), cleaned.rfind(close)) {
                    if end > start {
                        if let Ok(v) = serde_json::from_str(&cleaned[start..=end]) {
                            return Ok(v);
                        }
                    }
                }
            }
            Err(serde_json::from_str::<serde_json::Value>("").unwrap_err())
        })
        .map_err(|e| format!("AI returned invalid JSON: {e}"))?;

    let array = find_question_array(&parsed)
        .ok_or("AI response did not contain a JSON array of questions")?;

    let questions: Vec<Question> = array
        .iter()
        .filter_map(|v| serde_json::from_value::<Question>(v.clone()).ok())
        .filter(|q: &Question| {
            q.answers.len() == 4 && q.explanations.len() == 4 && q.correct_index < 4
        })
        .map(|mut q| {
            q.answers = q.answers.iter().map(|a| strip_label_prefix(a)).collect();
            q.explanations = q.explanations.iter().map(|e| strip_label_prefix(e)).collect();
            q
        })
        .collect();
    if questions.is_empty() {
        return Err("AI response contained no valid, complete questions".into());
    }
    Ok(questions)
}

/// Strip a redundant leading letter-label some models add to answers/
/// explanations (e.g. "B provides connectivity...", "C. is a physical...")
/// even when told not to — the app already renders that letter itself.
/// Only strips B/C/D as a bare leading word (never legitimate English), or
/// any letter A-D followed by punctuation (unambiguous label marker either
/// way) — never a bare leading "A " alone, since that's commonly a real
/// article ("A virtual network is...") and stripping it would be wrong.
fn strip_label_prefix(s: &str) -> String {
    let trimmed = s.trim_start();
    let mut chars = trimmed.char_indices();
    let Some((_, c)) = chars.next() else {
        return s.to_string();
    };
    let upper = c.to_ascii_uppercase();
    if !('A'..='D').contains(&upper) {
        return s.to_string();
    }
    let rest = &trimmed[c.len_utf8()..];
    let mut rest_chars = rest.chars();
    if let Some(next_c) = rest_chars.next() {
        if matches!(next_c, '.' | ':' | ')' | '-' | '\u{2013}' | '\u{2014}') {
            let after_punct = &rest[next_c.len_utf8()..];
            return after_punct.trim_start().to_string();
        }
    }
    if upper != 'A' && rest.starts_with(char::is_whitespace) {
        return rest.trim_start().to_string();
    }
    s.to_string()
}

fn http() -> Result<reqwest::Client, String> {
    // 5 minutes per HTTP request is already generous for local inference;
    // the old 30-minute ceiling let a wedged request sit silently for far
    // too long before the user saw any error.
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())
}

// ---------- local AI (Ollama) ----------

/// A JSON Schema describing exactly the shape we need. Passing this as
/// Ollama's `format` (rather than the bare string "json") constrains the
/// model's decoding to the exact field names, types, and array lengths —
/// not just "some valid JSON" — which is what actually prevents schema
/// mismatches like missing fields or the wrong number of answers.
fn question_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "answers": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 4,
                    "maxItems": 4
                },
                "correct_index": { "type": "integer", "minimum": 0, "maximum": 3 },
                "explanations": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 4,
                    "maxItems": 4
                }
            },
            "required": ["question", "answers", "correct_index", "explanations"]
        }
    })
}

fn emit_gen_progress(app: &tauri::AppHandle, msg: &str) {
    use tauri::Emitter;
    let _ = app.emit("gen-progress", msg.to_string());
}

async fn call_ollama_once(
    model: &str,
    prompt: &str,
    format: Option<serde_json::Value>,
    num_predict: i64,
) -> Result<String, String> {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": false,
        // Reasoning models (e.g. Qwen3) burn most of their token budget on
        // hidden "thinking" text unless told not to — that text isn't part
        // of our JSON answer and just crowds out the real output.
        "think": false,
        // num_ctx must cover the prompt AND the generated answer together.
        // num_predict is bounded (scaled to the question count by the
        // caller) rather than left uncapped, so a struggling model can't
        // ramble indefinitely and leave the user staring at a frozen screen.
        "options": { "num_ctx": 16384, "num_predict": num_predict, "temperature": 0.6 }
    });
    if let Some(f) = format {
        body["format"] = f;
    }
    let resp = http()?
        .post(format!("{OLLAMA_BASE}/api/chat"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Local AI took too long to respond (over 5 minutes) — try fewer questions, \
a smaller model, or fewer models at once"
                    .to_string()
            } else {
                "Local AI is not running — open Settings and click \"Install & start local AI\""
                    .to_string()
            }
        })?;
    let status = resp.status();
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Local AI response unreadable: {e}"))?;
    if !status.is_success() {
        let msg = json["error"].as_str().unwrap_or("unknown error");
        return Err(format!("Local AI error ({status}): {msg}"));
    }
    if json["done_reason"].as_str() == Some("length") {
        return Err("ran out of room mid-answer".into());
    }
    json["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "Local AI returned no text".into())
}

/// Try the strict schema first (best guarantee of a usable result); if the
/// model can't produce it (older Ollama, or a model that ignores schema
/// constraints), fall back to plain JSON mode, then to no constraint at all,
/// relying on the tolerant text parser. Each attempt is only made if the
/// previous one truly failed, so well-behaved models pay no extra cost.
/// Progress is emitted before each attempt so the UI can show real status
/// instead of a generic spinner with no information behind it.
async fn call_ollama(
    app: &tauri::AppHandle,
    model: &str,
    prompt: &str,
    count: usize,
) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() || model.contains(' ') {
        return Err(format!(
            "\"{model}\" is not a valid model name — pick a model from the list in Settings"
        ));
    }
    // Roughly 350 tokens per question (question + 4 answers + 4
    // explanations) is generous; bounded between 2K and 12K so neither a
    // tiny request starves nor a huge one runs unbounded.
    let num_predict = ((count as i64) * 350).clamp(2000, 12000);
    let attempts: [(Option<serde_json::Value>, &str); 3] = [
        (Some(question_schema()), "starting"),
        (
            Some(serde_json::Value::String("json".into())),
            "retrying with plain JSON mode",
        ),
        (None, "retrying without format constraints"),
    ];
    let mut last_err = String::new();
    for (fmt, phase) in attempts {
        emit_gen_progress(app, &format!("{model}: {phase}…"));
        match call_ollama_once(model, prompt, fmt, num_predict).await {
            Ok(text) => match extract_json_array(&text) {
                Ok(_) => return Ok(text),
                Err(e) => last_err = e,
            },
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

fn models_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?.join("models");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn run_hidden(program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
}

/// Kill any already-running Ollama process so a location/config change takes
/// effect on the next start. Best-effort — failures are ignored, since the
/// process may simply not be running.
fn kill_ollama() {
    #[cfg(target_os = "windows")]
    {
        let _ = run_hidden("taskkill", &["/IM", "ollama.exe", "/F"]);
        let _ = run_hidden("taskkill", &["/IM", "ollama app.exe", "/F"]);
    }
    #[cfg(target_os = "macos")]
    {
        let _ = run_hidden("pkill", &["-x", "ollama"]);
        let _ = run_hidden("pkill", &["-x", "Ollama"]);
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = run_hidden("pkill", &["-x", "ollama"]);
    }
}

/// Best-effort persistence of OLLAMA_MODELS so Ollama's own autostart (set
/// up by its installer, outside our control) also uses the right folder —
/// belt-and-suspenders on top of us always passing the env var explicitly
/// whenever *we* spawn `ollama serve` ourselves.
fn persist_models_env(dir_str: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = run_hidden("setx", &["OLLAMA_MODELS", dir_str]);
    }
    #[cfg(target_os = "macos")]
    {
        // Session-scoped: applies to GUI-launched apps (including Ollama's
        // own menu-bar autostart) for the current login session. Does not
        // survive a reboot on its own, but our app re-asserts it via
        // ensure_models_location() on every launch, and we always pass the
        // env var directly when we spawn `ollama serve` ourselves regardless.
        let _ = run_hidden("launchctl", &["setenv", "OLLAMA_MODELS", dir_str]);
    }
}

/// Make ExamGo AI's own data folder the home of all AI models.
/// Runs once (marker file): stops any running Ollama, moves already-downloaded
/// models over, and persists OLLAMA_MODELS for every future Ollama start.
async fn ensure_models_location(
    app: &tauri::AppHandle,
    emit: &impl Fn(&str),
) -> Result<PathBuf, String> {
    let dir = models_dir(app)?;
    let marker = data_dir(app)?.join("models_localized");
    if marker.exists() {
        return Ok(dir);
    }
    emit("Moving AI model storage into the ExamGo AI folder (one-time)…");

    // stop any running Ollama so the new location takes effect
    let _ = tauri::async_runtime::spawn_blocking(kill_ollama).await;
    sleep_ms(1500).await;

    // move any previously downloaded models so they don't re-download.
    // Ollama uses ~/.ollama on every OS; HOME covers macOS/Linux, USERPROFILE covers Windows.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok();
    if let Some(home) = home {
        let old = PathBuf::from(home).join(".ollama").join("models");
        for sub in ["blobs", "manifests"] {
            let from = old.join(sub);
            let to = dir.join(sub);
            if from.exists() && !to.exists() {
                let _ = fs::rename(&from, &to);
            }
        }
    }

    // persist for future Ollama starts (autostart, manual, ours)
    let dir_str = dir.to_string_lossy().to_string();
    let _ = tauri::async_runtime::spawn_blocking(move || persist_models_env(&dir_str)).await;

    fs::write(&marker, b"1").map_err(|e| e.to_string())?;
    Ok(dir)
}

fn find_ollama_exe() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(local)
                .join("Programs")
                .join("Ollama")
                .join("ollama.exe");
            if p.exists() {
                return Some(p);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // GUI-launched apps don't reliably inherit a shell's PATH additions
        // (e.g. Homebrew's /opt/homebrew/bin), so check the common install
        // locations directly before falling back to a PATH lookup.
        for candidate in ["/opt/homebrew/bin/ollama", "/usr/local/bin/ollama"] {
            let p = PathBuf::from(candidate);
            if p.exists() {
                return Some(p);
            }
        }
        // Installed via direct download (no Homebrew) — the CLI binary lives
        // inside the app bundle rather than on a standard PATH. Don't assume
        // the exact internal layout; search for it.
        if let Ok(out) = std::process::Command::new("find")
            .args([
                "/Applications/Ollama.app",
                "-type",
                "f",
                "-name",
                "ollama",
                "-perm",
                "+111",
            ])
            .output()
        {
            if let Some(first) = String::from_utf8_lossy(&out.stdout).lines().next() {
                if !first.is_empty() {
                    return Some(PathBuf::from(first));
                }
            }
        }
    }
    let on_path = std::process::Command::new("ollama")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if on_path {
        Some(PathBuf::from("ollama"))
    } else {
        None
    }
}

async fn ollama_running() -> bool {
    match http() {
        Ok(client) => client
            .get(format!("{OLLAMA_BASE}/api/tags"))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        Err(_) => false,
    }
}

async fn ollama_models() -> Vec<String> {
    let Ok(client) = http() else {
        return Vec::new();
    };
    let Ok(resp) = client
        .get(format!("{OLLAMA_BASE}/api/tags"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    else {
        return Vec::new();
    };
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    json["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn model_matches(installed: &str, wanted: &str) -> bool {
    installed == wanted || installed == format!("{wanted}:latest")
}

#[derive(Serialize)]
struct LocalAiStatus {
    installed: bool,
    running: bool,
    model: String,
    model_ready: bool,
    models: Vec<String>,
    localized: bool,
}

#[tauri::command]
async fn local_ai_status(app: tauri::AppHandle) -> Result<LocalAiStatus, String> {
    let settings = get_settings(app.clone())?;
    let model = default_model(&settings);
    let installed = find_ollama_exe().is_some();
    let running = ollama_running().await;
    let models = if running { ollama_models().await } else { Vec::new() };
    let model_ready = models.iter().any(|m| model_matches(m, &model));
    let localized = data_dir(&app)?.join("models_localized").exists();
    Ok(LocalAiStatus {
        installed,
        running,
        model,
        model_ready,
        models,
        localized,
    })
}

/// Install Ollama automatically where a reliable, scriptable path exists;
/// otherwise fail with clear manual-install instructions rather than doing
/// something fragile (e.g. downloading and running an unsigned installer).
async fn install_ollama(emit: &impl Fn(&str)) -> Result<(), String> {
    emit("Installing Ollama (one-time, ~700 MB)… this can take a few minutes.");

    #[cfg(target_os = "windows")]
    let status = tauri::async_runtime::spawn_blocking(|| {
        std::process::Command::new("winget")
            .args([
                "install",
                "-e",
                "--id",
                "Ollama.Ollama",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ])
            .status()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("could not run winget: {e}"))?;

    #[cfg(target_os = "macos")]
    let status = {
        let have_brew = std::process::Command::new("sh")
            .args(["-c", "command -v brew"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if have_brew {
            tauri::async_runtime::spawn_blocking(|| {
                std::process::Command::new("brew")
                    .args(["install", "ollama"])
                    .status()
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("could not run brew: {e}"))?
        } else {
            // No Homebrew — download Ollama's official app bundle directly
            // and unpack it into /Applications, same "no manual steps"
            // experience as the Windows winget path.
            emit("No Homebrew found — downloading Ollama directly from ollama.com…");
            tauri::async_runtime::spawn_blocking(|| {
                std::process::Command::new("sh")
                    .args([
                        "-c",
                        "set -e; \
                         tmp=$(mktemp -d); \
                         curl -fsSL -o \"$tmp/Ollama-darwin.zip\" https://ollama.com/download/Ollama-darwin.zip; \
                         ditto -xk \"$tmp/Ollama-darwin.zip\" /Applications/; \
                         rm -rf \"$tmp\"",
                    ])
                    .status()
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("could not download Ollama: {e}"))?
        }
    };

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let status = tauri::async_runtime::spawn_blocking(|| {
        std::process::Command::new("sh")
            .args(["-c", "curl -fsSL https://ollama.com/install.sh | sh"])
            .status()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("could not run the Ollama install script: {e}"))?;

    if !status.success() && find_ollama_exe().is_none() {
        return Err(
            "Ollama installation failed — install it manually from ollama.com/download \
and try again"
                .into(),
        );
    }
    Ok(())
}

#[tauri::command]
async fn setup_local_ai(app: tauri::AppHandle, model: Option<String>) -> Result<String, String> {
    use tauri::Emitter;
    let emit = |msg: &str| {
        let _ = app.emit("local-ai-progress", msg.to_string());
    };

    let settings = get_settings(app.clone())?;
    let target = match model {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => default_model(&settings),
    };
    if target.contains(' ') {
        return Err(format!(
            "\"{target}\" is not a valid model name — use a name from ollama.com/library, \
e.g. qwen3:4b"
        ));
    }

    // 1. install Ollama if missing
    if find_ollama_exe().is_none() {
        install_ollama(&emit).await?;
    }

    // 1.5 make sure models live in the ExamGo AI folder
    let model_store = ensure_models_location(&app, &emit).await?;

    // 2. start the server if not running
    if !ollama_running().await {
        emit("Starting local AI server…");
        if let Some(exe) = find_ollama_exe() {
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("serve");
            cmd.env("OLLAMA_MODELS", &model_store);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let _ = cmd.spawn();
        }
        for _ in 0..30 {
            if ollama_running().await {
                break;
            }
            sleep_ms(1000).await;
        }
        if !ollama_running().await {
            return Err("local AI server did not start — try restarting the app".into());
        }
    }

    // 3. pull the model if missing (with progress)
    let have = ollama_models().await;
    if !have.iter().any(|m| model_matches(m, &target)) {
        emit(&format!(
            "Downloading AI model \"{target}\" (one-time)…"
        ));
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .post(format!("{OLLAMA_BASE}/api/pull"))
            .json(&serde_json::json!({"model": target}))
            .send()
            .await
            .map_err(|e| format!("model download failed to start: {e}"))?;

        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut last_pct: i64 = -1;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("model download interrupted: {e}"))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf.drain(..=pos);
                if line.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if let Some(err) = v["error"].as_str() {
                    return Err(format!("model download failed: {err}"));
                }
                let status = v["status"].as_str().unwrap_or("");
                if let (Some(total), Some(done)) = (v["total"].as_u64(), v["completed"].as_u64())
                {
                    let pct = (done * 100 / total.max(1)) as i64;
                    if pct != last_pct {
                        last_pct = pct;
                        emit(&format!("Downloading \"{target}\" — {pct}%"));
                    }
                } else if !status.is_empty() {
                    emit(status);
                }
            }
        }
    }

    emit(&format!("Model \"{target}\" is ready!"));
    Ok("ready".into())
}

#[tauri::command]
fn open_models_folder(app: tauri::AppHandle) -> Result<String, String> {
    let dir = models_dir(&app)?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    // If this path is a reparse point/junction (e.g. an AppData virtualization
    // shim from a sandboxed environment), resolve it to its real target —
    // the shell can fail to follow such links even though the path "exists".
    let real = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let real_str = real
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string();
    #[cfg(target_os = "windows")]
    {
        // Goes through the OS shell association (same path a double-click
        // takes) instead of invoking explorer.exe's own argv parsing, which
        // is more reliable for arbitrary folder paths.
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", &real_str]);
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd.spawn()
            .map_err(|e| format!("could not open folder ({real_str}): {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&real_str)
            .spawn()
            .map_err(|e| format!("could not open folder ({real_str}): {e}"))?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&real_str)
            .spawn()
            .map_err(|e| format!("could not open folder ({real_str}): {e}"))?;
    }
    Ok(real_str)
}

#[tauri::command]
async fn delete_model(model: String) -> Result<(), String> {
    let client = http()?;
    let resp = client
        .delete(format!("{OLLAMA_BASE}/api/delete"))
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .map_err(|e| format!("could not reach local AI: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("could not remove model ({})", resp.status()))
    }
}

async fn sleep_ms(ms: u64) {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let _ = tx.send(());
    });
    let _ = rx.await;
}

// ---------- generation ----------

async fn generate_with(
    app: tauri::AppHandle,
    model: String,
    exam: String,
    count: usize,
    difficulty: String,
) -> (String, Result<Vec<Question>, String>) {
    let prompt = build_prompt(&exam, count, &difficulty);
    let result = call_ollama(&app, &model, &prompt, count)
        .await
        .and_then(|t| extract_json_array(&t));
    if result.is_ok() {
        emit_gen_progress(&app, &format!("{model}: done"));
    }
    (model, result)
}

#[tauri::command]
fn cancel_generation(state: tauri::State<GenState>) -> Result<(), String> {
    if let Some(handle) = state.0.lock().map_err(|e| e.to_string())?.take() {
        handle.abort();
    }
    Ok(())
}

#[tauri::command]
async fn generate_quiz(
    app: tauri::AppHandle,
    state: tauri::State<'_, GenState>,
    exam: String,
    set_name: String,
    count: usize,
    models: Vec<String>,
    difficulty: Option<String>,
) -> Result<QuestionSet, String> {
    let difficulty = difficulty.unwrap_or_default();
    if exam.trim().is_empty() {
        return Err("exam name is empty".into());
    }
    if set_name.trim().is_empty() {
        return Err("set name is empty".into());
    }
    if models.is_empty() {
        return Err("select at least one AI model".into());
    }
    let count = count.clamp(1, 100);

    let existing = data_dir(&app)?
        .join("sets")
        .join(format!("{}.json", sanitize_name(&set_name)));
    if existing.exists() {
        return Err(format!(
            "a question set named \"{set_name}\" already exists — pick another name"
        ));
    }

    // split the requested count across the selected models (they run in parallel)
    let per = count.div_ceil(models.len());
    let tasks: Vec<_> = models
        .iter()
        .map(|m| generate_with(app.clone(), m.clone(), exam.clone(), per, difficulty.clone()))
        .collect();
    let (abort_handle, abort_reg) = AbortHandle::new_pair();
    *state.0.lock().map_err(|e| e.to_string())? = Some(abort_handle);
    let results = match Abortable::new(futures::future::join_all(tasks), abort_reg).await {
        Ok(r) => r,
        Err(_) => return Err("generation cancelled".into()),
    };
    *state.0.lock().map_err(|e| e.to_string())? = None;

    let mut questions: Vec<Question> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (model, result) in results {
        match result {
            Ok(mut qs) => questions.append(&mut qs),
            Err(e) => errors.push(format!("{model}: {e}")),
        }
    }
    if questions.is_empty() {
        return Err(format!("all models failed — {}", errors.join(" | ")));
    }
    questions.truncate(count);

    let set = QuestionSet {
        name: set_name.trim().to_string(),
        exam: exam.trim().to_string(),
        providers: models,
        created_at: now_ms(),
        questions,
        attempts: Vec::new(),
    };
    save_set(&app, &set)?;
    Ok(set)
}

#[tauri::command]
fn record_attempt(
    app: tauri::AppHandle,
    name: String,
    correct: usize,
    total: usize,
) -> Result<Vec<Attempt>, String> {
    let mut set = load_set(app.clone(), name)?;
    set.attempts.push(Attempt {
        correct,
        total,
        at: now_ms(),
    });
    save_set(&app, &set)?;
    Ok(set.attempts)
}

#[tauri::command]
fn import_quiz(
    app: tauri::AppHandle,
    exam: String,
    set_name: String,
    raw_text: String,
) -> Result<QuestionSet, String> {
    if exam.trim().is_empty() {
        return Err("exam name is empty".into());
    }
    if set_name.trim().is_empty() {
        return Err("set name is empty".into());
    }
    let existing = data_dir(&app)?
        .join("sets")
        .join(format!("{}.json", sanitize_name(&set_name)));
    if existing.exists() {
        return Err(format!(
            "a question set named \"{set_name}\" already exists — pick another name"
        ));
    }
    let questions = extract_json_array(&raw_text).map_err(|e| {
        format!(
            "{e} — make sure you pasted the AI's full JSON reply \
(it must be a JSON array of question objects)"
        )
    })?;
    let set = QuestionSet {
        name: set_name.trim().to_string(),
        exam: exam.trim().to_string(),
        providers: vec!["imported".into()],
        created_at: now_ms(),
        questions,
        attempts: Vec::new(),
    };
    save_set(&app, &set)?;
    Ok(set)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(GenState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            get_progress,
            save_progress,
            list_sets,
            load_set,
            delete_set,
            generate_quiz,
            cancel_generation,
            import_quiz,
            record_attempt,
            local_ai_status,
            setup_local_ai,
            delete_model,
            open_models_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
