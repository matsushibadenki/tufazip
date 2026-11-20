"use strict";
const $ = (id) => document.getElementById(id);
const icons = {
  chevrons: "M8 9l4-4 4 4 M8 15l4 4 4-4",
  down: "M6 9l6 6 6-6",
  pack: "M4 13v7h16v-7 M12 3v12m-4-4 4 4 4-4",
  unpack: "M4 13v7h16v-7 M12 15V3m-4 4 4-4 4 4",
  lock: "M6 10h12v11H6z M9 10V7a3 3 0 016 0v3",
  panel: "M3 5h18v14H3z M14 5v14",
  folder: "M3 7V5h6l2 3h10v11H3z",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z M15 12a3 3 0 11-6 0 3 3 0 016 0",
  add: "M12 3v12m-4-4 4 4 4-4 M4 15v5h16v-5",
  info: "M22 12a10 10 0 11-20 0 10 10 0 0120 0 M12 11v5 M12 7h.01",
  globe:
    "M22 12a10 10 0 11-20 0 10 10 0 0120 0 M2 12h20 M12 2a18 18 0 010 20 18 18 0 010-20",
};
document.querySelectorAll("[data-icon]").forEach((el) => {
  el.innerHTML =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="' +
    icons[el.dataset.icon] +
    '"/></svg>';
});

const native = window.__TAURI__;
const panel = native?.window.getCurrentWindow().label === "settings";
document.body.classList.toggle("panel", panel);
let doc,
  words,
  displayedFiles = "",
  transientError = "",
  writes = Promise.resolve();
const defaults = {
  mode: "compress",
  format: "zip",
  level: 2,
  splitMb: 0,
  password: "",
  confirm: "",
  clean: true,
  verify: false,
  separate: false,
  language: "auto",
};
function tooltip(el, text) {
  el.title = text;
  el.setAttribute("aria-label", text);
}
function localizedError(message) {
  const code = message.split(":")[0];
  if (words[code])
    return (
      words[code] +
      (code === "unsupportedItem" ? message.slice(code.length) : "")
    );
  if (/password|decrypt/i.test(message)) return words.badPassword;
  return words.failed + " " + message;
}
function render(next) {
  if (doc && next.revision < doc.revision) return;
  doc = next;
  const opt = doc.options;
  const lang =
    opt.language === "auto" ? navigator.language.slice(0, 2) : opt.language;
  words = labels[lang] || labels.en;
  document.documentElement.lang =
    words === labels.zh ? "zh-CN" : words === labels.ja ? "ja" : "en";
  document
    .querySelectorAll("[data-t]")
    .forEach((el) => (el.textContent = words[el.dataset.t]));
  document
    .querySelectorAll("[data-tip]")
    .forEach((el) => tooltip(el, words[el.dataset.tip]));
  const extract = opt.mode === "extract";
  const local = !extract && opt.format === "local";
  document.body.classList.toggle("extract", extract);
  document.body.classList.toggle("local-compress", local);
  document
    .querySelectorAll("[data-mode]")
    .forEach((el) =>
      el.setAttribute("aria-pressed", String(el.dataset.mode === opt.mode)),
    );
  for (const key of ["clean", "verify", "separate"]) $(key).checked = opt[key];
  const detectOption = $("format").querySelector('option[value="detect"]');
  if (extract) {
    if (detectOption) detectOption.textContent = words.detect;
    else $("format").add(new Option(words.detect, "detect"));
  } else {
    detectOption?.remove();
  }
  $("format").value = extract ? "detect" : opt.format;
  $("level").value = opt.level;
  $("level").style.setProperty("--fill", (opt.level / 3) * 100 + "%");
  $("level").setAttribute("aria-valuetext", words.levels[opt.level]);
  $("split").value = opt.splitMb;
  for (const key of ["password", "confirm"]) {
    if (document.activeElement !== $(key) && $(key).value !== opt[key])
      $(key).value = opt[key];
  }
  $("password").placeholder = extract ? words.optional : "";
  $("aes").classList.toggle("active", !!opt.password);
  $("localInfo").hidden = !local;
  $("language").value = opt.language;
  $("settingTitle").textContent = extract
    ? words.extractSettings
    : words.method + " · " + words.levels[opt.level];
  $("ticks").replaceChildren(
    ...words.levels.map((name, i) => {
      const label = document.createElement("span");
      label.textContent = name;
      label.classList.toggle("selected", i === opt.level);
      return label;
    }),
  );
  tooltip($("detach"), panel ? words.dock : words.detach);
  tooltip(
    $("eye"),
    $("password").type === "password" ? words.show : words.hide,
  );
  $("dropLabel").textContent = extract ? words.archive : words.files;
  $("editor").hidden = doc.floating && !panel;
  $("redock").hidden = !doc.floating || panel;
  const fileKey = JSON.stringify([doc.inputs, lang, doc.busy]);
  if (fileKey !== displayedFiles) {
    displayedFiles = fileKey;
    $("files").replaceChildren(
      ...doc.inputs.map((path) => {
        const li = document.createElement("li"),
          label = document.createElement("span"),
          remove = document.createElement("button");
        label.textContent = path.split("/").pop();
        label.title = path;
        remove.className = "icon";
        remove.textContent = "×";
        tooltip(remove, words.remove + " " + label.textContent);
        remove.onclick = () => call("remove_input", { path });
        li.append(label, remove);
        return li;
      }),
    );
  }
  $("files").hidden = !doc.inputs.length;
  const error = transientError || doc.error;
  $("notice").textContent = error
    ? localizedError(error)
    : doc.busy
      ? doc.current || words.chooseDest
      : doc.output
        ? words.done + " · " + doc.output.split("/").pop()
        : "";
  $("notice").classList.toggle("error", !!error && error !== "cancelled");
  $("notice").hidden = !$("notice").textContent;
  $("progress").hidden = !doc.busy;
  if (doc.total > 1) {
    $("progress").max = doc.total;
    $("progress").value = doc.position;
  } else $("progress").removeAttribute("value");
  $("action").textContent = doc.busy
    ? words.cancel
    : doc.output
      ? words.reveal
      : extract
        ? words.unpack
        : words.start;
  document
    .querySelectorAll("input,select,button")
    .forEach((el) => (el.disabled = !!doc.busy));
  $("format").disabled = doc.busy || extract;
  $("action").disabled = !doc.busy && !doc.inputs.length;
}

async function call(name, args = {}) {
  transientError = "";
  try {
    if (!native) throw new Error("desktop");
    await native.core.invoke(name, args);
  } catch (e) {
    transientError = String(e.message || e);
    render(doc);
  }
}
function configure(key, value) {
  writes = writes.then(() => call("configure", { key, value }));
  return writes;
}
for (const key of ["format", "password", "confirm", "language"])
  $(key).onchange = (e) => configure(key, e.target.value);
for (const key of ["clean", "verify", "separate"])
  $(key).onchange = (e) => configure(key, e.target.checked);
$("level").oninput = (e) => configure("level", Number(e.target.value));
$("split").onchange = (e) => configure("splitMb", Number(e.target.value));
document
  .querySelectorAll("[data-mode]")
  .forEach((el) => (el.onclick = () => configure("mode", el.dataset.mode)));
$("eye").onclick = () => {
  const show = $("password").type === "password";
  $("password").type = $("confirm").type = show ? "text" : "password";
  tooltip($("eye"), show ? words.hide : words.show);
};
$("chooseFiles").onclick = () => call("choose_inputs", { folders: false });
$("chooseFolders").onclick = () => call("choose_inputs", { folders: true });
$("detach").onclick = () => call("float_settings", { floating: !panel });
$("redock").onclick = () => call("float_settings", { floating: false });
$("action").onclick = async () => {
  await writes;
  await call(doc.busy ? "cancel" : doc.output ? "reveal" : "start_job");
};
async function init() {
  render({ revision: -1, options: defaults, inputs: [], busy: false });
  if (!native) return;
  await native.event.listen("document", (evt) => {
    transientError = "";
    render(evt.payload);
  });
  await native.window
    .getCurrentWindow()
    .onDragDropEvent((evt) =>
      document.body.classList.toggle(
        "drag",
        ["enter", "over"].includes(evt.payload.type),
      ),
    );
  render(await native.core.invoke("snapshot"));
}
init().catch((e) => {
  transientError = String(e);
  render(doc);
});
