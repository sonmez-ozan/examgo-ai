const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const views = ["home", "generate", "import", "quiz", "results", "settings", "about"];

let quiz = null; // { set, index, correctCount }
let setupRunning = false;

function showView(name) {
  for (const v of views) {
    $(`view-${v}`).classList.toggle("hidden", v !== name);
  }
}

function setStatus(el, text, kind) {
  el.textContent = text;
  el.className = `status ${kind || ""}`;
}

function escapeHtml(s) {
  return String(s)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

// ---------- level / XP progress ----------

const LEVEL_MAX = 50;
const XP_PER_QUESTION = 2.5; // 50 XP per 20 questions completed

function buildLevelThresholds() {
  // thresholds[level - 1] = cumulative XP required to REACH that level.
  // Level 1 starts at 0 XP; level 2 needs 100; every level after that
  // needs 1.2x the XP the previous level needed, stacked on the total.
  const thresholds = [0];
  let increment = 100;
  let cumulative = 0;
  for (let lvl = 2; lvl <= LEVEL_MAX; lvl++) {
    cumulative += increment;
    thresholds.push(cumulative);
    increment = Math.round(increment * 1.2);
  }
  return thresholds;
}
const LEVEL_THRESHOLDS = buildLevelThresholds();

function levelForXp(xp) {
  let lvl = 1;
  for (let i = 1; i < LEVEL_THRESHOLDS.length; i++) {
    if (xp >= LEVEL_THRESHOLDS[i]) lvl = i + 1;
    else break;
  }
  return lvl;
}

let progress = { xp: 0, level: 1 };

async function loadProgress() {
  try {
    progress = await invoke("get_progress");
  } catch (e) {
    console.error("could not load level progress:", e);
  }
  renderLevelBadge();
}

function renderLevelBadge() {
  const badge = $("level-badge");
  const ring = $("level-ring-fill");
  const level = progress.level;
  $("level-num").textContent = level;

  const atMax = level >= LEVEL_MAX;
  badge.classList.toggle("is-max", atMax);

  if (atMax) {
    ring.style.strokeDashoffset = "0";
    badge.title = `Level ${LEVEL_MAX} — max level reached! ${progress.xp} XP total.`;
    return;
  }
  const floor = LEVEL_THRESHOLDS[level - 1];
  const ceil = LEVEL_THRESHOLDS[level];
  const pct = Math.max(0, Math.min(1, (progress.xp - floor) / (ceil - floor)));
  ring.style.strokeDashoffset = String(100 - pct * 100);
  badge.title = `Level ${level} — ${progress.xp - floor} / ${ceil - floor} XP to level ${level + 1}`;
}

const LEVEL_UP_LINES = [
  "Your neurons just unionized for better hours.",
  "Somewhere, a textbook feels a little lighter.",
  "You didn't just level up — you out-studied yourself.",
  "New personal best: brain cells, fully caffeinated.",
  "That's another notch on the scholar belt.",
  "Your future self just sent a thank-you note.",
  "Knowledge acquired. Ego, appropriately proportional.",
  "The quiz gods are impressed. Mildly. But impressed.",
  "You're officially harder to stump than yesterday.",
  "Plot twist: you're actually good at this.",
];

let levelUpQueue = [];
let levelUpShowing = false;

function queueLevelUps(fromLevel, toLevel) {
  for (let lvl = fromLevel + 1; lvl <= toLevel; lvl++) levelUpQueue.push(lvl);
  if (!levelUpShowing) showNextLevelUp();
}

function showNextLevelUp() {
  const level = levelUpQueue.shift();
  if (level === undefined) {
    levelUpShowing = false;
    $("levelup-overlay").classList.add("hidden");
    return;
  }
  levelUpShowing = true;
  const atMax = level >= LEVEL_MAX;
  $("levelup-num").textContent = level;
  $("levelup-title").textContent = atMax
    ? `Level ${level} — Grand Scholar!`
    : `Level ${level} unlocked!`;
  $("levelup-message").textContent = atMax
    ? "You've hit the top of the ladder. There's nowhere left to climb but the exam itself — go get it."
    : LEVEL_UP_LINES[(level - 1) % LEVEL_UP_LINES.length];
  $("levelup-overlay").classList.remove("hidden");
  $("levelup-continue").focus();
}

$("levelup-continue").addEventListener("click", showNextLevelUp);
document.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape" && levelUpShowing) showNextLevelUp();
});

async function awardXp(amount) {
  if (amount <= 0) return;
  const prevLevel = progress.level;
  progress.xp += amount;
  progress.level = Math.min(LEVEL_MAX, levelForXp(progress.xp));
  renderLevelBadge();
  try {
    await invoke("save_progress", { progress });
  } catch (e) {
    console.error("could not save level progress:", e);
  }
  if (progress.level > prevLevel) queueLevelUps(prevLevel, progress.level);
}

// ---------- model catalog ----------

const MODEL_CATALOG = [
  {
    tag: "gemma3:4b",
    label: "Gemma3-4B",
    size: "3.3 GB",
    req: "6 GB RAM · any modern PC",
    desc: "Google's compact model — installed automatically as the default",
  },
  {
    tag: "qwen3:4b",
    label: "Qwen3-4B",
    size: "2.6 GB",
    req: "6 GB RAM · any modern PC",
    desc: "Balanced quality and speed",
  },
  {
    tag: "qwen3:8b",
    label: "Qwen3-8B",
    size: "5.2 GB",
    req: "10 GB RAM · mid-range PC or better",
    desc: "Noticeably better questions than 4B models",
  },
  {
    tag: "llama3.2:3b",
    label: "Llama3.2-3B",
    size: "2.0 GB",
    req: "4 GB RAM · runs on older laptops",
    desc: "Fastest and lightest option",
  },
  {
    tag: "gemma3:12b",
    label: "Gemma3-12B",
    size: "8.1 GB",
    req: "12 GB RAM · strong PC (16 GB total)",
    desc: "Highest quality in this list",
  },
  {
    tag: "mistral:7b",
    label: "Mistral-7B",
    size: "4.4 GB",
    req: "8 GB RAM · mid-range PC",
    desc: "Solid all-rounder",
  },
];

function displayName(tag) {
  const base = tag.replace(/:latest$/, "");
  const hit = MODEL_CATALOG.find((m) => m.tag === base);
  if (hit) return hit.label;
  // prettify unknown tags: "llama3.1:8b" -> "Llama3.1-8B"
  const [family, variant] = base.split(":");
  const fam = family.charAt(0).toUpperCase() + family.slice(1);
  return variant ? `${fam}-${variant.toUpperCase()}` : fam;
}

// ---------- sidebar / sets ----------

function sourceLabel(providers) {
  if (!providers || providers.length === 0) return "Unknown source";
  if (providers.includes("imported")) return "Imported";
  return providers.map(displayName).join(", ");
}

function buildSetItem(s) {
  const item = document.createElement("div");
  item.className = "set-item";
  const date = new Date(s.created_at).toLocaleDateString();
  const attempts = (s.attempts || []).slice(-1); // latest attempt only
  const history = attempts
    .map((a) => {
      const pct = a.total ? Math.round((a.correct / a.total) * 100) : 0;
      return `<div class="attempt-row" title="${new Date(a.at).toLocaleString()}"><span class="attempt-pct">${pct}%</span><span class="attempt-frac">${a.correct}/${a.total}</span></div>`;
    })
    .join("");
  item.innerHTML = `
    <div class="set-body">
      <div class="set-name" title="${escapeHtml(s.name)}">${escapeHtml(s.name)}</div>
      <div class="set-meta">${escapeHtml(sourceLabel(s.providers))} · ${date}</div>
    </div>
    <div class="set-history">${history}</div>
    <button class="set-delete" title="Delete set">✕</button>`;
  item.addEventListener("click", () => startQuizFromSet(s.name));
  item.querySelector(".set-delete").addEventListener("click", async (ev) => {
    ev.stopPropagation();
    if (!confirm(`Delete question set "${s.name}"?`)) return;
    try {
      await invoke("delete_set", { name: s.name });
    } catch (e) {
      alert(`Delete failed: ${e}`);
    }
    refreshSets();
  });
  return item;
}

async function refreshSets() {
  const sidebarList = $("set-list");
  const homeList = $("home-set-list");
  sidebarList.innerHTML = "";
  if (homeList) homeList.innerHTML = "";
  let sets = [];
  try {
    sets = await invoke("list_sets");
  } catch (e) {
    const msg = `<div class="empty-note">Could not load sets: ${e}</div>`;
    sidebarList.innerHTML = msg;
    if (homeList) homeList.innerHTML = msg;
    return;
  }
  if (sets.length === 0) {
    const msg =
      '<div class="empty-note">No question sets yet — create your first quiz!</div>';
    sidebarList.innerHTML = msg;
    if (homeList) homeList.innerHTML = msg;
    return;
  }
  for (const s of sets) {
    sidebarList.appendChild(buildSetItem(s));
    if (homeList) homeList.appendChild(buildSetItem(s));
  }
}

// ---------- local AI status + model lists ----------

async function getLocalAiStatus() {
  try {
    return await invoke("local_ai_status");
  } catch {
    return { installed: false, running: false, model: "gemma3:4b", model_ready: false, models: [], localized: true };
  }
}

function renderGenerateModels(st) {
  const box = $("model-list");
  if (!st.running || st.models.length === 0) {
    box.innerHTML =
      '<div class="empty-note">No local AI models yet — setup runs automatically, or open Settings to manage models.</div>';
    return;
  }
  box.innerHTML = st.models
    .map(
      (m, i) => `
      <label class="check"
        ><input type="checkbox" name="model" value="${escapeHtml(m)}" ${i === 0 ? "checked" : ""} />
        <span class="check-body">
          <span class="check-name">${escapeHtml(displayName(m))}</span>
          <span class="check-sub">${escapeHtml(m)}</span>
        </span></label>`,
    )
    .join("");
}

function renderCatalog(st) {
  const box = $("catalog-list");
  const installed = new Set(st.models);
  const isInstalled = (name) =>
    installed.has(name) || installed.has(`${name}:latest`);
  box.innerHTML = MODEL_CATALOG.map((m) => {
    const have = isInstalled(m.tag);
    const locateBtn = `<button type="button" class="btn-secondary model-locate" title="Open the folder where this model is stored">📂</button>`;
    const action = have
      ? `${locateBtn}<button type="button" class="btn-secondary model-remove" data-model="${m.tag}">Remove</button>`
      : `<button type="button" class="btn-secondary model-pull" data-model="${m.tag}">Download</button>`;
    return `
      <div class="model-row${have ? " installed" : ""}">
        <div class="model-info">
          <span class="model-name">${m.label}${have ? ' <span class="model-badge">✔ installed</span>' : ""}</span>
          <span class="model-desc">${m.size} — ${m.desc}</span>
          <span class="model-req">Requires: ${m.req}</span>
        </div>
        <div class="model-action">${action}</div>
      </div>`;
  }).join("");

  box.querySelectorAll(".model-pull").forEach((btn) =>
    btn.addEventListener("click", () => pullModel(btn.dataset.model)),
  );
  box.querySelectorAll(".model-locate").forEach((btn) =>
    btn.addEventListener("click", async () => {
      try {
        const path = await invoke("open_models_folder");
        console.log("models folder:", path);
      } catch (e) {
        alert(String(e));
      }
    }),
  );
  box.querySelectorAll(".model-remove").forEach((btn) =>
    btn.addEventListener("click", async () => {
      if (!confirm(`Remove model "${btn.dataset.model}" from this PC?`)) return;
      try {
        await invoke("delete_model", { model: btn.dataset.model });
      } catch (e) {
        alert(String(e));
      }
      refreshLocalAi();
    }),
  );
}

async function refreshLocalAi() {
  const st = await getLocalAiStatus();
  const el = $("local-ai-status");
  if (st.running && st.models.length > 0) {
    setStatus(el, `✔ Ready — ${st.models.length} model(s) installed and running.`, "ok");
  } else if (st.running) {
    setStatus(el, "Server running — no models downloaded yet.", "busy");
  } else if (st.installed) {
    setStatus(el, "Ollama is installed but not running.", "busy");
  } else {
    setStatus(el, "Not set up yet — setup runs automatically, or click below.", "busy");
  }
  renderGenerateModels(st);
  renderCatalog(st);
  return st;
}

listen("local-ai-progress", (ev) => {
  setStatus($("local-ai-progress"), ev.payload, "busy");
  setStatus($("gen-status"), ev.payload, "busy");
});

// Real per-model/per-attempt progress from generate_quiz — takes over from
// the generic rotating messages the moment it starts arriving.
listen("gen-progress", (ev) => {
  genLastRealProgress = ev.payload;
  if (genStartedAt) renderGenStatus(genLastRealProgress);
});

async function pullModel(model) {
  if (setupRunning) return;
  setupRunning = true;
  setStatus($("local-ai-progress"), `Preparing "${model}"…`, "busy");
  try {
    await invoke("setup_local_ai", { model });
    setStatus($("local-ai-progress"), `✔ "${model}" is ready.`, "ok");
  } catch (e) {
    setStatus($("local-ai-progress"), String(e), "error");
  } finally {
    setupRunning = false;
    refreshLocalAi();
  }
}

$("local-ai-setup").addEventListener("click", () => autoSetup(true));

$("custom-model-pull").addEventListener("click", () => {
  const model = $("custom-model").value.trim();
  if (!model) {
    setStatus($("local-ai-progress"), "Type a model name first, e.g. llama3.1:8b", "error");
    return;
  }
  pullModel(model);
});

// First-run automation: install Ollama + default model without any clicks.
async function autoSetup(force = false) {
  if (setupRunning) return;
  const st = await refreshLocalAi();
  if (!force && st.running && st.models.length > 0 && st.localized) return; // already good
  setupRunning = true;
  setStatus($("gen-status"), "Setting up free local AI (one-time)…", "busy");
  setStatus($("local-ai-progress"), "Setting up free local AI (one-time)…", "busy");
  try {
    await invoke("setup_local_ai", {});
    setStatus($("gen-status"), "✔ Local AI is ready — create your first quiz!", "ok");
    setStatus($("local-ai-progress"), "✔ Local AI is ready.", "ok");
  } catch (e) {
    setStatus($("gen-status"), String(e), "error");
    setStatus($("local-ai-progress"), String(e), "error");
  } finally {
    setupRunning = false;
    refreshLocalAi();
  }
}

// ---------- generate ----------

const GEN_MESSAGES = [
  "Consulting the exam objectives…",
  "Drafting tricky questions…",
  "Inventing plausible wrong answers…",
  "Writing explanations that actually teach…",
  "Double-checking the correct answers…",
  "Shuffling A, B, C and D…",
  "Brewing a fresh batch of questions…",
  "Thinking hard so you don't have to (yet)…",
  "Grading its own homework…",
  "Almost there — polishing the wording…",
];
let genMsgTimer = null;
let genElapsedTimer = null;
let genStartedAt = 0;
let genLastRealProgress = "";

function genElapsedText() {
  const secs = Math.max(0, Math.round((Date.now() - genStartedAt) / 1000));
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return m > 0 ? `${m}:${String(s).padStart(2, "0")}` : `${s}s`;
}

function renderGenStatus(text) {
  setStatus($("gen-status"), `${text} · ${genElapsedText()} elapsed`, "busy");
}

function startGenAnimation() {
  $("gen-cancel").classList.remove("hidden");
  $("gen-loading-icon").classList.remove("hidden");
  genStartedAt = Date.now();
  genLastRealProgress = "";
  let i = 0;
  renderGenStatus(GEN_MESSAGES[0]);
  genMsgTimer = setInterval(() => {
    // Real progress from the backend (which model/attempt is running) takes
    // priority over the generic rotating flavor text.
    if (genLastRealProgress) {
      renderGenStatus(genLastRealProgress);
      return;
    }
    i = (i + 1) % GEN_MESSAGES.length;
    renderGenStatus(GEN_MESSAGES[i]);
  }, 3000);
  // Tick the elapsed-time counter every second so it's visibly alive even
  // between the 3-second message rotations.
  genElapsedTimer = setInterval(() => {
    renderGenStatus(genLastRealProgress || $("gen-status").textContent.split(" · ")[0]);
  }, 1000);
}

function stopGenAnimation() {
  $("gen-cancel").classList.add("hidden");
  $("gen-loading-icon").classList.add("hidden");
  if (genMsgTimer) {
    clearInterval(genMsgTimer);
    genMsgTimer = null;
  }
  if (genElapsedTimer) {
    clearInterval(genElapsedTimer);
    genElapsedTimer = null;
  }
  genLastRealProgress = "";
}

$("generate-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const models = [...document.querySelectorAll('input[name="model"]:checked')].map(
    (c) => c.value,
  );
  const status = $("gen-status");
  if (models.length === 0) {
    setStatus(status, "Select at least one AI model (or wait for setup to finish).", "error");
    return;
  }
  const btn = $("gen-submit");
  btn.disabled = true;
  startGenAnimation();
  try {
    const set = await invoke("generate_quiz", {
      exam: $("gen-exam").value,
      setName: $("gen-name").value,
      count: parseInt($("gen-count").value, 10) || 20,
      models,
      difficulty: $("gen-difficulty").value || null,
    });
    setStatus(status, `Created "${set.name}" with ${set.questions.length} questions.`, "ok");
    $("generate-form").reset();
    $("gen-count").value = 20;
    await refreshSets();
    const st = await getLocalAiStatus();
    renderGenerateModels(st);
    startQuiz(set);
  } catch (e) {
    stopGenAnimation();
    setStatus(status, String(e), "error");
  } finally {
    btn.disabled = false;
    stopGenAnimation();
  }
});

$("gen-cancel").addEventListener("click", async () => {
  try {
    if (genMsgTimer) {
      clearInterval(genMsgTimer);
      genMsgTimer = null;
    }
    await invoke("cancel_generation");
    setStatus($("gen-status"), "Cancelling…", "busy");
  } catch (e) {
    setStatus($("gen-status"), String(e), "error");
  }
});

// ---------- quiz ----------

async function startQuizFromSet(name) {
  try {
    const set = await invoke("load_set", { name });
    startQuiz(set);
  } catch (e) {
    alert(`Could not open set: ${e}`);
  }
}

function startQuiz(set) {
  quiz = { set, index: 0, correctCount: 0 };
  $("quiz-title").textContent = set.name;
  $("quiz-exam").textContent = set.exam;
  renderQuizHistory();
  showView("quiz");
  renderQuestion();
}

function renderQuizHistory() {
  const list = $("quiz-history-list");
  const attempts = [...(quiz.set.attempts || [])].reverse(); // newest first
  if (attempts.length === 0) {
    list.innerHTML = '<div class="empty-note">No attempts yet — this is your first run!</div>';
    return;
  }
  list.innerHTML = attempts
    .map((a) => {
      const pct = a.total ? Math.round((a.correct / a.total) * 100) : 0;
      const when = new Date(a.at).toLocaleString([], {
        dateStyle: "short",
        timeStyle: "short",
      });
      return `<div class="attempt-row"><span class="attempt-pct">${pct}%</span><span class="attempt-frac">${a.correct}/${a.total}</span><span class="attempt-date">${when}</span></div>`;
    })
    .join("");
}

function renderQuestion() {
  const { set, index } = quiz;
  const q = set.questions[index];
  $("quiz-progress").textContent = `Question ${index + 1} / ${set.questions.length}`;
  $("quiz-question").textContent = q.question;

  const answersBox = $("quiz-answers");
  answersBox.innerHTML = "";
  const letters = ["A", "B", "C", "D"];
  q.answers.forEach((answer, i) => {
    const btn = document.createElement("button");
    btn.className = "answer-btn";
    btn.innerHTML = `<span class="answer-letter">${letters[i]}</span>${escapeHtml(answer)}`;
    btn.addEventListener("click", () => answerChosen(i));
    answersBox.appendChild(btn);
  });

  $("quiz-explanations").classList.add("hidden");
  $("quiz-explanations").innerHTML = "";
  $("quiz-next").classList.add("hidden");
}

function answerChosen(chosen) {
  const { set, index } = quiz;
  const q = set.questions[index];
  const correct = q.correct_index;
  const buttons = [...$("quiz-answers").querySelectorAll(".answer-btn")];
  const letters = ["A", "B", "C", "D"];

  buttons.forEach((b) => (b.disabled = true));
  buttons[correct].classList.add("correct");
  if (chosen !== correct) buttons[chosen].classList.add("wrong");
  if (chosen === correct) quiz.correctCount++;

  const box = $("quiz-explanations");
  box.innerHTML = "";
  const verdict = document.createElement("div");
  verdict.className = `verdict ${chosen === correct ? "good" : "bad"}`;
  verdict.textContent =
    chosen === correct
      ? "✔ Correct!"
      : `✘ Wrong — the correct answer is ${letters[correct]}.`;
  box.appendChild(verdict);

  q.explanations.forEach((text, i) => {
    const div = document.createElement("div");
    div.className = "explanation";
    if (i === correct) div.classList.add("correct");
    else if (i === chosen) div.classList.add("chosen-wrong");
    div.innerHTML = `<b>${letters[i]}.</b>${escapeHtml(text)}`;
    box.appendChild(div);
  });
  box.classList.remove("hidden");

  const next = $("quiz-next");
  next.textContent =
    index + 1 < set.questions.length ? "Next question →" : "See results →";
  next.classList.remove("hidden");
}

$("quiz-next").addEventListener("click", () => {
  quiz.index++;
  if (quiz.index < quiz.set.questions.length) {
    renderQuestion();
  } else {
    showResults();
  }
});

async function showResults() {
  const total = quiz.set.questions.length;
  const pct = Math.round((quiz.correctCount / total) * 100);
  $("result-score").textContent = `${pct}%`;
  $("result-detail").textContent = `You answered ${quiz.correctCount} of ${total} questions correctly on "${quiz.set.name}".`;
  showView("results");
  try {
    const attempts = await invoke("record_attempt", {
      name: quiz.set.name,
      correct: quiz.correctCount,
      total,
    });
    quiz.set.attempts = attempts;
    renderQuizHistory();
    refreshSets();
  } catch (e) {
    console.error("could not record attempt:", e);
  }
  await awardXp(Math.round(total * XP_PER_QUESTION));
}

$("result-retry").addEventListener("click", () => startQuiz(quiz.set));
$("result-home").addEventListener("click", () => showView("home"));

// ---------- import from pasted text ----------

const DIFFICULTY_GUIDANCE = {
  easy: "Keep every question at EASY difficulty: fundamental definitions, single-concept recall, and terminology — the kind of question that checks basic familiarity with a topic.",
  medium:
    "Keep every question at MEDIUM difficulty: applied, scenario-based questions that require combining two or three related concepts, similar to the bulk of questions on a real certification exam.",
  hard: "Keep every question at HARD difficulty: complex, multi-step scenarios that require analysis, trade-off judgment, or knowledge of edge cases — comparable to the hardest questions on the real exam.",
  "": "Vary the difficulty naturally across the set: include some fundamental recall questions, mostly applied scenario questions, and a few genuinely hard ones — the same spread a real certification exam would have.",
};

function importPrompt() {
  const exam = $("imp-exam").value.trim() || "[ENTER EXAM NAME]";
  const count = Math.min(100, Math.max(1, parseInt($("imp-count").value, 10) || 20));
  const difficulty = $("imp-difficulty").value;
  const label = difficulty ? ` at ${difficulty} difficulty` : "";
  const guidance = DIFFICULTY_GUIDANCE[difficulty] || DIFFICULTY_GUIDANCE[""];
  return `You are an expert certification-exam item writer with deep subject-matter expertise in "${exam}". Generate exactly ${count} multiple-choice practice questions for this exam${label}.

Guidelines:
- Cover a broad, representative range of the exam's official objectives; do not repeat the same narrow sub-topic more than twice.
- Write questions the way they appear on real certification exams: clear and unambiguous, with scenario-based phrasing where that fits the topic. Exactly one answer must be correct.
- All 4 answer options must be plausible and similar in length and style. Wrong answers should reflect realistic misconceptions or common mistakes, never be obviously silly or off-topic.
- Vary which position (0-3) holds the correct answer across questions so the pattern isn't predictable — do not favor any one letter.
- Never repeat the same question wording or scenario twice in the set.
- ${guidance}

Respond with ONLY a JSON array — no markdown fences, no commentary, no text before or after the array. Each element must be an object with exactly these fields:
  "question": string — the question text
  "answers": array of exactly 4 answer strings
  "correct_index": integer 0-3 — index of the correct answer
  "explanations": array of exactly 4 strings — for each answer, explain concisely why it is correct or incorrect, teaching the underlying concept so the learner improves even from a wrong guess

Each explanation should be 1-3 sentences, specific and educational — avoid generic filler like "this is correct because it is the best option."`;
}

function renderImportPrompt() {
  $("prompt-box").textContent = importPrompt();
}

$("goto-import").addEventListener("click", () => {
  renderImportPrompt();
  showView("import");
});
$("goto-generate").addEventListener("click", () => showView("generate"));
$("imp-exam").addEventListener("input", renderImportPrompt);
$("imp-count").addEventListener("input", renderImportPrompt);
$("imp-difficulty").addEventListener("change", renderImportPrompt);

async function copyImportPrompt() {
  const status = $("copy-status");
  try {
    await navigator.clipboard.writeText(importPrompt());
    setStatus(status, "✔ Prompt copied — now paste it into any AI chat.", "ok");
    return true;
  } catch {
    const range = document.createRange();
    range.selectNodeContents($("prompt-box"));
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
    setStatus(status, "Press Ctrl+C to copy the selected prompt.", "busy");
    return false;
  }
}

$("import-setup").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  renderImportPrompt();
  await copyImportPrompt();
});

$("copy-prompt").addEventListener("click", copyImportPrompt);

$("imp-file").addEventListener("change", () => {
  const f = $("imp-file").files[0];
  if (!f) return;
  const reader = new FileReader();
  reader.onload = () => {
    $("imp-text").value = reader.result;
    setStatus($("imp-status"), `Loaded "${f.name}" — now click Create question set.`, "ok");
  };
  reader.onerror = () => setStatus($("imp-status"), `Could not read "${f.name}".`, "error");
  reader.readAsText(f);
});

$("import-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const status = $("imp-status");
  const btn = $("imp-submit");
  btn.disabled = true;
  setStatus(status, "Checking and saving your questions…", "busy");
  try {
    const set = await invoke("import_quiz", {
      exam: $("imp-exam").value,
      setName: $("imp-name").value,
      rawText: $("imp-text").value,
    });
    setStatus(status, `Created "${set.name}" with ${set.questions.length} questions.`, "ok");
    $("import-form").reset();
    await refreshSets();
    startQuiz(set);
  } catch (e) {
    setStatus(status, String(e), "error");
  } finally {
    btn.disabled = false;
  }
});

// ---------- navigation ----------

document.querySelectorAll(".nav-btn[data-view]").forEach((btn) => {
  btn.addEventListener("click", () => {
    const view = btn.dataset.view;
    if (view === "settings") refreshLocalAi();
    if (view === "import") renderImportPrompt();
    showView(view);
  });
});

// ---------- init ----------

refreshSets();
showView("home");
autoSetup();
loadProgress();
