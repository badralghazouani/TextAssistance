// Suggestion popup. Rust captures the text and calls the AI; this file shows
// the corrections, lets you turn each one on or off, and sends back the text
// to paste.
//
// Suggestion offsets are UTF-16 code units, which is exactly how JavaScript
// indexes strings, so they can be used with slice() directly.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const els = {
  status: $("status"),
  resultView: $("result-view"),
  preview: $("preview"),
  rewriteBox: $("rewrite-box"),
  suggestions: $("suggestions"),
  errorView: $("error-view"),
  errorMessage: $("error-message"),
  keyForm: $("key-form"),
  keyProvider: $("key-provider"),
  keyInput: $("key-input"),
  keySave: $("key-save"),
  openAccessibility: $("open-accessibility"),
  footerInfo: $("footer-info"),
  copy: $("copy"),
  replace: $("replace"),
  close: $("close"),
  modeButtons: [...document.querySelectorAll("button[data-mode]")],
};

const state = {
  text: "",
  paragraphs: [],
  off: new Set(), // ids ("line:index") of suggestions the user switched off
  mode: "check", // "check" or "rewrite"
  lastCheck: null,
  request: 0, // ignore answers to requests that were superseded
};

// ---------- session lifecycle ----------

function start(view) {
  state.request++;
  state.text = view.text || "";
  state.paragraphs = [];
  state.off.clear();
  state.mode = "check";
  state.lastCheck = null;
  setActiveMode(null);
  els.footerInfo.textContent = "";

  if (view.error) {
    showError(view.error);
    return;
  }
  showChecking();
  runCheck();
}

async function runCheck() {
  const request = ++state.request;
  try {
    const result = await invoke("check", { text: state.text });
    if (request !== state.request) return;
    state.lastCheck = result;
    renderCheck(result);
  } catch (err) {
    if (request === state.request) showError(err);
  }
}

async function runRewrite(mode) {
  if (!state.text) return;
  // Clicking the active mode again goes back to the corrections.
  if (state.mode === "rewrite" && activeMode() === mode && state.lastCheck) {
    setActiveMode(null);
    renderCheck(state.lastCheck);
    return;
  }
  const request = ++state.request;
  state.mode = "rewrite";
  setActiveMode(mode);
  els.resultView.hidden = false;
  els.errorView.hidden = true;
  els.preview.hidden = true;
  els.suggestions.innerHTML = "";
  els.rewriteBox.hidden = false;
  els.rewriteBox.value = "Rewriting…";
  els.rewriteBox.disabled = true;
  els.status.textContent = "Rewriting…";
  setActionsEnabled(false);
  try {
    const result = await invoke("rewrite", { text: state.text, mode });
    if (request !== state.request) return;
    els.rewriteBox.value = result.text;
    els.rewriteBox.disabled = false;
    els.status.textContent = "Rewritten. You can edit it before replacing.";
    els.footerInfo.textContent = result.footer;
    setActionsEnabled(true);
    els.rewriteBox.focus();
  } catch (err) {
    if (request === state.request) showError(err);
  }
}

// ---------- rendering ----------

function showChecking() {
  els.errorView.hidden = true;
  els.resultView.hidden = false;
  els.rewriteBox.hidden = true;
  els.preview.hidden = false;
  els.preview.classList.add("loading");
  els.preview.textContent = state.text;
  els.suggestions.innerHTML = "";
  els.status.textContent = "Checking…";
  setActionsEnabled(false);
  setRewriteEnabled(true);
}

function renderCheck(result) {
  state.mode = "check";
  state.paragraphs = result.paragraphs;
  els.errorView.hidden = true;
  els.resultView.hidden = false;
  els.rewriteBox.hidden = true;
  els.preview.hidden = false;
  els.preview.classList.remove("loading");
  els.preview.innerHTML = "";
  els.suggestions.innerHTML = "";
  els.footerInfo.textContent = result.footer;

  let count = 0;
  state.text.split("\n").forEach((line, lineNo) => {
    if (lineNo > 0) els.preview.append("\n");
    const report = result.paragraphs.find((p) => p.line === lineNo);
    if (!report) {
      els.preview.append(line);
      return;
    }
    let cursor = 0;
    report.suggestions.forEach((s, i) => {
      if (s.start < cursor) return; // overlaps the previous one
      const id = `${lineNo}:${i}`;
      count++;
      els.preview.append(line.slice(cursor, s.start));

      const mark = document.createElement("span");
      mark.className = "sug";
      mark.dataset.id = id;
      mark.title = s.explanation;
      const del = document.createElement("del");
      del.textContent = line.slice(s.start, s.end);
      const ins = document.createElement("ins");
      ins.textContent = s.replacement;
      mark.append(del, ins);
      mark.addEventListener("click", () => toggle(id));
      els.preview.append(mark);
      cursor = s.end;

      const item = document.createElement("li");
      item.dataset.id = id;
      item.title = "Click to keep or skip this change";
      item.innerHTML = `<span class="kind"></span><span><span class="change"></span><br><span class="why"></span></span>`;
      item.querySelector(".kind").textContent = s.kind;
      item.querySelector(".change").textContent = `${s.original} → ${s.replacement}`;
      item.querySelector(".why").textContent = s.explanation;
      item.addEventListener("click", () => toggle(id));
      els.suggestions.append(item);
    });
    els.preview.append(line.slice(cursor));
  });

  if (count === 0) {
    els.status.textContent = "No issues found";
    const ok = document.createElement("p");
    ok.className = "empty";
    ok.textContent = "✓ Looks good. Nothing to correct.";
    els.suggestions.replaceChildren(ok);
    els.replace.disabled = true;
    els.copy.disabled = false;
  } else {
    els.status.textContent = `${count} suggestion${count > 1 ? "s" : ""}. Click one to skip it.`;
    setActionsEnabled(true);
  }
  setRewriteEnabled(true);
}

function toggle(id) {
  if (state.off.has(id)) state.off.delete(id);
  else state.off.add(id);
  document.querySelectorAll(`[data-id="${id}"]`).forEach((el) => {
    el.classList.toggle("off", state.off.has(id));
  });
}

function showError(err) {
  const error =
    typeof err === "string" ? { kind: "other", message: err } : err || { kind: "other", message: "Something went wrong." };
  els.resultView.hidden = true;
  els.errorView.hidden = false;
  els.errorMessage.textContent = error.message;
  els.keyForm.hidden = error.kind !== "no_key";
  els.openAccessibility.hidden = error.kind !== "accessibility";
  els.status.textContent = "";
  setActionsEnabled(false);
  setRewriteEnabled(false);
  if (error.kind === "no_key") {
    if (error.provider) els.keyProvider.value = error.provider;
    els.keyInput.value = "";
    els.keyInput.focus();
  }
}

// ---------- actions ----------

function finalText() {
  if (state.mode === "rewrite") return els.rewriteBox.value;
  return state.text
    .split("\n")
    .map((line, lineNo) => {
      const report = state.paragraphs.find((p) => p.line === lineNo);
      if (!report) return line;
      let cursor = 0;
      const accepted = report.suggestions.filter((s, i) => {
        if (s.start < cursor) return false;
        cursor = s.end;
        return !state.off.has(`${lineNo}:${i}`);
      });
      for (const s of accepted.reverse()) {
        line = line.slice(0, s.start) + s.replacement + line.slice(s.end);
      }
      return line;
    })
    .join("\n");
}

async function replace() {
  if (els.replace.disabled) return;
  setActionsEnabled(false);
  await invoke("apply", { text: finalText() });
}

async function copy() {
  await invoke("copy_text", { text: finalText() });
  els.status.textContent = "Copied to the clipboard.";
}

function close() {
  state.request++;
  invoke("cancel");
}

async function saveKey() {
  els.keySave.disabled = true;
  try {
    await invoke("save_api_key", { provider: els.keyProvider.value, key: els.keyInput.value });
    els.keyInput.value = "";
    if (state.text) {
      showChecking();
      runCheck();
    } else {
      showError({ kind: "other", message: "Key saved. Press the shortcut in any text field to check it." });
    }
  } catch (err) {
    showError(err);
  } finally {
    els.keySave.disabled = false;
  }
}

// ---------- helpers ----------

function setActionsEnabled(enabled) {
  els.replace.disabled = !enabled;
  els.copy.disabled = !enabled;
}

function setRewriteEnabled(enabled) {
  els.modeButtons.forEach((b) => (b.disabled = !enabled));
}

function setActiveMode(mode) {
  els.modeButtons.forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
}

function activeMode() {
  return els.modeButtons.find((b) => b.classList.contains("active"))?.dataset.mode;
}

// ---------- wiring ----------

els.replace.addEventListener("click", replace);
els.copy.addEventListener("click", copy);
els.close.addEventListener("click", close);
els.keySave.addEventListener("click", saveKey);
els.openAccessibility.addEventListener("click", () => invoke("open_accessibility_settings"));
els.modeButtons.forEach((b) => b.addEventListener("click", () => runRewrite(b.dataset.mode)));

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    close();
  } else if (e.key === "Enter" && !e.shiftKey) {
    const inField = e.target === els.rewriteBox || e.target === els.keyInput;
    if (e.target === els.keyInput) {
      e.preventDefault();
      saveKey();
    } else if (!inField || e.metaKey || e.ctrlKey) {
      e.preventDefault();
      replace();
    }
  }
});

listen("session", (event) => start(event.payload));
invoke("get_session").then((view) => view && start(view));
