# Translation engine (tp_engine)

`helper\engine` is the library that turns a TRANSLATE request into a translation. It finds the
llama.cpp runtime and the Qwen3-4B model on disk, starts `llama-server` on the first request (GPU
when enough VRAM is free, else CPU), builds the prompt, post-processes the output, caches results
and reports one `Status` stream per request. It implements design sections D.4 and D.5. The app
(`helper\app`) is its only user; `src\bin\evalprompt.rs` is the E1 evaluation tool.

## 1. Interface for the app

```rust
let engine = Engine::start(cfg, Box::new(|u: Update| ...), Box::new(|s: EngineState| ...));
let h = engine.handle();          // Clone + Send, usable from any thread
h.submit(job); h.cancel(key); h.clear_session(session);
h.set_wow_present(false);         // starts the 60 s unload countdown
h.set_backend(BackendPref::Cpu);  // unloads after the request in flight; next start uses it
h.unload_now();                   // tray "Unload translator now"
h.set_remote(Some(cfg));          // remote service with the player's key (section 13); None: local
engine.shutdown();                // or drop: kills llama-server, joins within 2 s
let json = tp_engine::selftest::run(&cfg);   // --selftest, never loads the model
```

- Both callbacks run on the engine thread. Keep them short (post a message to the UI thread).
- Statuses per job, in order: `Queued{ahead}` (only while waiting behind another job),
  `Loading{first_gpu_use}` (the server is starting for this job), `Translating`, zero or more
  `Partial` (CPU only, at most 5 per second, placeholders already restored), then one final
  status: `Done`, `Same`, `Nothing`, `Failed(kind)`, `LowMemory` or `NotInstalled`.
- The text of `Done` and `Partial` (2026-10-19, L3): a link name the addon sent with a colour
  comes back as a coloured mark, `0x02 0x04 RRGGBB name 0x03` (`tp_bridge::marks::span`,
  upper-case hex); every other span is plain text. `tp_bridge::marks::plain` gives the visible
  text (the clipboard uses it), `marks::split` the coloured runs (the overlay).
- `Nothing`, `Same` (Hangul text to `ko`), cached results and `NotInstalled` are answered at
  submit, without a model call.
- Re-submitting a key that is still waiting or in flight is ignored (it keeps its place). After
  it finished, the same request is served from the cache; `no_cache` skips the cache read.
- `Job.urgent` (2026-10-19, the conversation window's reply card): the player waits for that
  translation to paste it, so it waits ahead of every job that is not urgent (section 5).
- `Failed` kinds: `Timeout` (request deadline or 30 s in the queue), `ServerDied` (start failure,
  crash, or the Error state), `BadOutput`, `Http`, `Overloaded` (pushed out of a full queue).
- `EngineState`: `NotInstalled`, `Unloaded`, `Loading`, `ReadyGpu`, `ReadyCpu`, `Busy`, `Error`.
  It describes the local engine only (a request to a remote service does not make it `Busy`).
- Remote services (2026-10-20, section 13): with `set_remote(Some(..))` a job goes to the service
  first (`Queued`, `Translating`, then `Done` or `Same`; no `Loading`, no `Partial`). When the
  service fails, the job runs the local path above and every later `Update` of it carries
  `fallback: Some(Fallback)` (provider, kind, HTTP status, the service's short message); every
  other update has `fallback: None`.
- `EngineEvent` (log only) carries numbers, enums and `&'static str`. The one exception is
  `ServerOutput { line: ServerLine }`: a llama-server line that passed the allowlist, sent only
  after a crash or a failed start. `ServerLine` can only be built by the filter (see section 4).
  Match `EngineEvent` with a wildcard arm; variants may be added (`InsistRetry`, E1b V2, is
  logged only when that switch is on).

## 2. Discovery

**Runtime.** Candidates, first match wins: `EngineConfig.runtime_dir` (the app passes `--runtime`,
else `settings.runtimeDir`), `<exe dir>\runtime`, `%LOCALAPPDATA%\Programs\Ollama\lib\ollama`. A
candidate needs `llama-server.exe` and `libllama-server-impl.dll`. Layout facts recorded with it:
`vulkan\ggml-vulkan.dll` present, and a CUDA folder (`cuda_v12` preferred for its wider driver
support, else `cuda_v13`).

**Model.** `EngineConfig.model_path` first (GGUF magic required, any digest accepted, logged as
`UnknownModel` when the digest is not known). Else every manifest under
`%OLLAMA_MODELS%\manifests` (or `%USERPROFILE%\.ollama\models\manifests`) is read; a layer of
media type `application/vnd.ollama.image.model` whose digest is in `model::KNOWN_DIGESTS`
(Qwen3-4B Q4_K_M, `sha256:7485fe6f...fdf5`) gives the blob `blobs\sha256-<hex>`, accepted when
it starts with `GGUF` and weighs 2.3 to 2.7 GB. Discovery runs at engine start and again (at most
every 30 s) while something is missing.

## 3. Flags probe

`llama-server.exe --help` runs once per exe identity (FNV-1a 64 of path, size and mtime); the
result is cached in `<data>\run\flags.json` as `{"version":1,"id":"<hash>","features":[...]}`:
flag names only, no path. Only listed flags are passed: `-lm none` (else `--no-mmap`), `--fit
off`, `--api-key-file` (else `--api-key`), `--no-ui` (else `--no-webui`), `--offline`, `-dev`,
`-np`, `-t`, `-tb`, `-ngl`. Missing `-m`, `--port` or `-c` puts the engine in the Error state.
Fixtures: `tests\fixtures\help_0_7_0.txt` (dev runtime, 0.7.0-dev) and `help_0_3_0.txt` (the
Ollama install, 0.3.0-dev, generated by running `--help` on it).

## 4. Device policy and server lifecycle

**Inputs** (`device.rs`). The adapter is the DXGI adapter with the most dedicated memory; it must
be a hardware adapter with at least 3 GB. Free VRAM comes from NVML (`System32\nvml.dll` loaded
at run time, the device matched by PCI device and subsystem ids) when it matches exactly one
device, else DXGI dedicated memory minus the PDH counter `GPU Adapter Memory(*)\Dedicated Usage`
of the adapter's LUID. Never the DXGI budget or `--list-devices` (both about 4.5 GB too high).
RAM is `ullAvailPhys`; physical cores come from `GetLogicalProcessorInformation`. On this PC NVML
read 5115 MB free while nvidia-smi read 5128 MB, and the PDH fallback agrees with NVML within
300 MB (unit test on the real machine).

**Choice** (`policy::choose`, pure). GPU when the preference is not `Cpu`, the session is not
blocked, and free VRAM is at least 2900 + `gpu_margin_mb` (1500): Vulkan when the runtime has it
(or forced `Vulkan`), else CUDA on NVIDIA when a CUDA folder exists. Otherwise CPU with
`clamp(physicalCores / 2, 2, 4)` threads (or `cpu_threads`), which needs 3600 MB of available RAM,
else every job gets `LowMemory`. No partial offload.

**Command line** (`server::launch_spec`):

```
llama-server.exe -m <gguf> --host 127.0.0.1 --port <free port> --api-key-file <data\run\key-<helper pid>.txt>
  -c 1024 -np 1 -t T -tb T -ngl <0|999> [-dev VulkanN] --fit off -lm none --no-ui --offline
```

- CPU: working directory = runtime root, `CUDA_VISIBLE_DEVICES=-1`, `-ngl 0`.
- Vulkan: working directory = `vulkan\`, `-ngl 999`, `-dev` = the `--list-devices` entry whose
  name matches the adapter (the only one when there is one). CUDA: working directory = the CUDA
  folder. The working directory selects the ggml GPU backend (measured).
- The API key is 32 hex characters from `BCryptGenRandom`, in a key file deleted when the server
  exits and when the engine stops.

**Process.** `CreateProcessW` with `CREATE_NO_WINDOW | CREATE_SUSPENDED | BELOW_NORMAL_PRIORITY_CLASS`;
the process is put in a new `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job, then resumed. Only stdin
(NUL) and the output pipe are inherited (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`). stdout and stderr
share one pipe drained by a thread; only lines containing `load_backend`, `offloaded`, `model
loaded`, `listening`, `error` or `failed` are kept (cut to 160 characters, last 50), and lines that
could echo a prompt or a completion (`<|`, `"prompt"`, `"content"`, `prompt:`, `content:`) are
dropped. The ring is logged only after a crash or a failed start.

**Readiness.** `GET /health` every 200 ms (500 ms probe timeout) until 200; deadline 60 s, 90 s on
the first Vulkan run (marker `<data>\run\vulkan_warmed` absent). Then one warm-up `/completion`
(`n_predict` 1, the target's system prefix: compiles Vulkan shaders, fills the prefix cache). A
successful Vulkan warm-up writes the empty marker file. Measured on this PC (CPU, 4 threads): HTTP
ready after 2.3 s, warm-up done at 3.2 s, first translation 5.1 s after the click.

**Per request.** `POST /completion` with `prompt, n_predict = min(256, 32 + input bytes),
temperature 0.2, top_p 0.8, top_k 20, seed 42, stop ["<|im_end|>"], cache_prompt, stream (CPU
only), grammar`. Deadline CPU `6 s + n_predict/8 s`, GPU `4 s + n_predict/40 s`, enforced by the
engine thread (the HTTP timeout is only a backstop, 2 s later). Each request runs on its own
short thread, so the engine thread never blocks on it.

**Failures.**

| Event | Result |
|---|---|
| Start fails (spawn, exit, health or warm-up deadline) on GPU | kill, one GPU failure counted, immediate CPU start for this job |
| Start fails on CPU | `Failed(ServerDied)`, Error state; the next start is allowed after 60 s |
| Request deadline | kill, `Failed(Timeout)`, counted as a crash; restart on the next job |
| Server exits during a request | `Failed(ServerDied)`, counted as a crash |
| 2 GPU failures in a session | CPU for the rest of the session (`GpuBlocked`) |
| 3 crashes within 10 minutes | Error state for 10 minutes |
| `set_backend` | clears the GPU block; unload after the request in flight |

**Unload.** After `idle_unload` (10 minutes) without a job unless `keep_loaded`; 60 s after
`set_wow_present(false)` (even with `keep_loaded`); on `unload_now`. `--sleep-idle-seconds` is not
used, so each start decides GPU or CPU again. **VRAM guard:** every 15 s while on GPU; below 700 MB
free, the session switches to CPU (unload after the request in flight).

## 5. Queue and cache

- FIFO: one job in flight (from server start to result), 8 waiting. A ninth waiting job pushes the
  oldest waiting one out with `Failed(Overloaded)`. Waiting jobs older than 30 s get
  `Failed(Timeout)`.
- Urgent jobs (`Job.urgent`, replies): `Queue::push` puts them after the waiting urgent jobs and
  ahead of every waiting job that is not urgent; they never interrupt the job in flight. A full
  queue drops its oldest job that is not urgent first. A waiting job that an urgent key merges
  into (same cache key) becomes urgent and moves forward. Every key whose position changed gets a
  new `Queued{ahead}`.
- Same cache key as a waiting job or the job in flight: merged, every key gets the same updates.
- Cancel and clear remove waiting keys; a key in flight gets nothing more, the request finishes
  and its result is still cached. A flight whose keys were all cancelled before any work started
  is dropped.
- Cache: in-memory LRU of 256 results (`Done` text or `Same`), never written to disk. Key =
  FNV-1a 64 of `PROMPT_VERSION` (3), model identity (digest, else size and mtime), target, protected
  text (default variant), the spans' contents and colours (`Protected::span_key`) and pairs, each
  field length-prefixed. The spans are in the key since 2026-10-19 (review fix, deviation 24):
  "wts {1}" for two different items, IDs or colours are two results. With a remote service the
  model identity is `remote:<provider>:<model>` (`remote:openai:<host[:port]>:<model>` for an
  OpenAI-compatible server), so a result of one service or model is never served for another;
  a result is stored under the identity of the route that produced it, and a local answer to a
  job that fell back is not stored (section 13).

## 6. Prompt, protection, post-processing

**Pre-processing** (`protect.rs`, `pipeline.rs`):
1. ChatML neutralization in text and pairs: `<|` becomes `< |`, `|>` becomes `| >`.
2. Line breaks and other control characters (except the marks 0x02-0x04) become spaces.
3. Marks (`tp_bridge::marks`, one grammar for the engine and the app): each complete
   `\x02...\x03` span is a marked span; since addon build 5 it may start with `\x04` and six hex
   digits, the link's colour, kept with the span (L3; the addon sends the colours in a COLOURS
   message and the app's scene writes them into the text before the job, `docs\BRIDGE.md` section
   3b, so the engine reads the same coloured form). Unterminated marks, stray `\x03` and empty
   spans stay or become plain text; a `\x02` inside an open span restarts it; a `\x04` anywhere
   else, or not followed by six hex digits, is dropped alone (`docs\BRIDGE.md` section 3a, the
   decode vectors of `tests\vectors\marks.json` are checked by `helper\bridge\tests\vectors.rs`).
4. **Verbatim tokens** (`verbatim.rs`, 2026-10-19, L4): in the plain text, URLs (`http://`,
   `https://`, `www.`, a domain with a path such as `discord.gg/...`, a known bare host such as
   `t.me` or `bit.ly`, a bare domain with a common top-level domain and a first label of 4
   characters or more; a top-level domain that is also a chat word, `me`, `to`, `it`, `de`..., needs
   3 labels, so `add.me` and `merci.de` stay words), e-mail addresses and IDs (a run of letters,
   digits and `_` with at least one letter and 2 digits, or a `_`, or a `#` and 3 to 6 digits; Han,
   kana and Hangul end a run, so an ID glued to Chinese text is found) are spans too. Full-width
   letters, digits and ASCII-range punctuation (Chinese input methods) count as their ASCII forms
   and the token is protected as written (`Ｋｉｗｉ４８２１３`; the Latin grammar would block it).
   Plain numbers, times (`20h30`, `20Uhr`, `20hs`, `20heures`, `20ч30`), levels (`lvl60`, `L60`,
   `niv60`, `nivel60`, `stufe60`, `лвл60`, `60лвл`, `ур60`), amounts and units (`x20`, `100g`,
   `5g50`, `5po50pa`, `50кк`, `10kkk`, `30min`, `2nd`, `20шт`), raid sizes (`10m`, `lf10m`,
   `icc25`, `10hm`, `icc10hm`), brackets (`3v3`), keys (`m15`), ratings (`pvp1800`, `rating2000`)
   and keep-allowed glossary terms (`aq40`) are not (`verbatim::game_number` lists the shapes in
   English, French, German, Spanish, Portuguese, Italian and Russian shorthand). Fact: in game the
   model's answer to a long Chinese yell ending with two IDs stopped before them; E1c below has the
   cause.
5. Spans become `{1}..{9}`, numbered in order of appearance: the marked spans first (at most 9),
   then the verbatim tokens while room is left; the others stay plain text. A `{N}` typed by a
   player is rewritten `{ N }` so it cannot pass for a placeholder.
6. No letters outside placeholders: `Nothing` (a line made only of IDs and URLs too). Mostly
   Hangul text with target `ko`: `Same`.
7. **Glossary expansion (E1b, V4):** each must-use pair whose meaning has one sense (no " / ")
   and no braces is replaced by its meaning in the message (whole words, ASCII case ignored, like
   the addon's matching); expanded pairs are left out of the prompt glossary. In the Brackets
   form only the text between `[...]` segments is expanded, so item names are never touched.

**Glossary policy** (`prompt::keep_allowed`, `prompt::must_use`). A pair is **keep-allowed** when
its term is a game abbreviation that players type as-is in every language: grouping and trade
(lf, lfg, lfm, lf`N`, lf`N`m, pug, wts, wtb, wtt, pst, inv, ah, boe, bop, ml, summ, lvl), roles and
fights (tank, heal, healer, dps, dd, mt, ot, hc, oom, cc, aoe, rez, rdy, dot, hot), chat signals
(afk, gg, wp, lol) and the short names of battlegrounds, dungeons, raids and cities (bg, wsg, ab,
av, dm, brd, ubrs, mc, sw, org, uc...), plus their plurals. The translation may keep those as
typed, because the addon's glossary row already shows their meaning. Every other pair is
**must-use**: chat words (thx, ty, pls, brb, gz, omw, idk, nvm, np...), false friends (cheap),
full names (stockade, armory) and user entries for other terms; the translation has to carry
their meaning. Names with an official localized form (dungeons, zones) must use that form; item
links are protected by placeholders and restored as the client shows them.

**Prompt** (`prompt::build`, `PROMPT_VERSION` 3; the prompt wording is the one of version 2,
version 3 only changed what becomes a placeholder and the cache key). Raw ChatML; thinking disabled with an empty
`<think>` block. The user turn is `Glossary:` (omitted without pairs), then `Message:` and the
text. Two system styles (`prompt::Style`):
- **Casual (default, V1):** the lead's measured GPU prompt, generalized to the target and to
  placeholders: "You translate chat messages from the online game World of Warcraft into
  {Language}. Write natural, casual {Language}, the way {a French player} would write in chat.
  Keep every token like {1} (item names), player names and game abbreviations (LFM, LFG, WTS,
  WTB, pst, DPS, BG, DM...) unchanged. Use the official {Language} World of Warcraft names when
  you know them. Reply with the {Language} translation only." The Brackets form says "item names
  in [brackets]" instead of the token wording.
- **Plain (E1, V0):** "You translate World of Warcraft chat messages into {Language}. Keep player
  names, numbers, and every token like {1} exactly as written. Use the glossary for game
  abbreviations. Reply with the translation only." (Brackets: "...anything inside [brackets]...",
  the measured probe text.)
- **Insist block (V2, off by default):** before `Message:`, "This message is not in {Language}
  yet. Translate all of it into {Language}; only tokens like {1}, player names and game
  abbreviations may stay as written. Write "{meaning}" for "{term}"." (must-use pairs only).

Golden strings for every style, variant and the insist block are in `prompt.rs` tests. The warm-up
sends the default style's system prefix.

**Grammars** (`grammar.rs`): Latin targets and ru block CJK, kana, Hangul and full-width forms;
zh and zh-TW block kana, Hangul and Cyrillic; ko blocks kana, Cyrillic and CJK ideographs.

**Post-processing** (`post.rs`, `pipeline::finish`):
1. Drop `<think>` blocks and tags (an output containing `<|im_` is `BadOutput`); drop the mark
   characters 0x02-0x04 (the model cannot forge a coloured span); join lines; strip labels
   (`Translation:`, `Traduction :` and their equivalents) and wrapping quotes the input did not
   have; cap at 3 x input characters + 40, a Han, kana, Hangul or full-width character counting 3
   (`post::length_cap`, 2026-10-19, L4: E1c below); empty is `BadOutput`.
2. Placeholders: each `{1..n}` exactly once and no other `{k}`, else one retry with
   `Variant::Brackets`.
3. **Bracket repair (Brackets answers):** when the output has as many `[...]` segments as the
   input, each segment that held an item name gets the original text back (E1b: a Korean link
   name came back translated in English).
4. **Same-language decision (E1b, V3):** the output is "unchanged" when it matches the message
   before or after expansion (casefolded letters and digits equal, or Levenshtein ratio at least
   0.9). An unchanged output is `Same` only with positive evidence that the source is already in
   the target language (`post::looks_like_target`): most letters in the target's script (Han
   without kana for zh); for Latin targets, frequent words of the target (lists for en, fr, de,
   es, it, pt; glossary terms and keep-allowed abbreviations not counted) outnumber those of
   every other Latin language and make up at least a quarter of the counted words. Without that
   evidence the output is shown as the translation (`Done`), so a foreign line is never labelled
   "Already in {language}".
5. Restore the spans: a span with a link colour as a coloured mark (`0x02 0x04 RRGGBB name
   0x03`), every other span as its text; in a Brackets answer, the wrapped spans are unwrapped
   and the coloured ones marked (the first occurrence outside a mark already written).

**Tuning and follow-ups.** `pipeline::Tuning` holds the E1b switches: style (V0 plain, V1
casual), `insist` (V2), `same_needs_evidence` (V3), `expand` (V4). `Tuning::DEFAULT` is
**v1+v3+v4**. A job makes at most two model calls (`pipeline::next`): the first, then either the
Brackets retry or (V2 only) the insist retry; an insist retry that fails keeps the first answer.
The worker and `evalprompt` share these functions.

## 7. Echo translator and selftest

`Translator::Echo` (`--translator echo`) never starts a process: `Loading` for 300 ms on the first
job (again after an idle unload), then `Translating`, then `Done(uppercase of the unmarked text)`
after 200 ms; a span with a link colour keeps it (written back as a coloured mark, like a real
translation), so E2E can check the coloured runs. It goes through the same queue, cache and
shortcuts as the real translator.

`selftest::run` returns JSON with `engine` (prompt version, default variant, tuning label),
`runtime`, `model`,
`flags` (runs `--help` once if `flags.json` has no entry; never loads the model), `gpu`
(adapters, picked adapter, free VRAM and its source), `ram`, `cpu` and `policy` (what a start
would choose now). It contains local paths (printed for the user, never logged by the engine).
`evalprompt --selftest` prints the same JSON. `selftest::run_with(cfg, discovery)` runs the same
report on a given discovery result; the unit test passes an empty one, so a plain `cargo test`
never starts llama-server whatever is installed on the machine.

## 8. Privacy guards

- Chat text travels only in `ChatText` (no Debug, Display, Clone or Serialize) and in the
  engine's internal status type, which has no Debug either.
- Log events have no text field; the server ring filter refuses prompt and completion lines.
- Nothing is written to disk except `flags.json` (flag names), the key file, the Vulkan marker
  and, with a remote service, `remote_usage.json` (a date and a character count). The API keys
  file `keys.bin` (DPAPI) is written by the app through `remote::keystore` (section 13).
- `evalprompt` reports hold ids, counts and timings only. `--show` (outputs) and `--show-raw`
  (raw answers of every attempt) print synthetic outputs to the console for a developer; they
  are never written to a file. Both corpora are synthetic.

## 9. Tests

From `helper\engine`:

| Command | What |
|---|---|
| `cargo test --offline` | 157 unit tests in the library (2 ignored: the child process of the job-object test, and `selftest_on_this_machine`, which may run `llama-server --help`), 5 in `evalprompt`, 1 ignored integration test; the remote-service tests (section 13) never leave 127.0.0.1 |
| `cargo test --offline --test real_server -- --ignored --nocapture` (env `TPROBE_LLAMA_RUNTIME`) | the public Engine API against the real llama-server on CPU, 60 s watchdog; since 2026-10-19 the synthetic line holds a coloured link and a synthetic ID, which must come back as a coloured mark and exactly as written (2026-10-19: first result after 13.9 s including the load, WoW running) |
| `cargo run --offline --release --bin evalprompt -- --runtime %TPROBE_LLAMA_RUNTIME% --cpu --threads 4 --out ..\..\tests\eval\out\report.json` | E1 with the shipped tuning, E1b and E1c lines included (`--tuning v0`, `v1+v3+v4`... for the others; `--samples ..\..\tests\eval\echo_samples.tsv` for the echo set); since E1d every `done` line also has its lengths outside the protected spans |

Unit tests cover: prompt goldens per target, variant and style, with and without glossary, and
the insist block; the glossary policy table; protect and restore (nesting, more than 9 spans,
unterminated marks, UTF-8, missing, duplicated, unknown and zero placeholders, typed `{1}`;
since 2026-10-19 coloured spans in both variants, a moved or repeated name in a Brackets answer,
verbatim tokens as placeholders, marks before tokens at the limit of 9, the span key); verbatim
token detection (the in-game shape glued to Chinese, URLs, addresses, BattleTags, `_` handles, and
60 number, time, level, unit and abbreviation shapes that stay plain); the length cap with wide
characters; marks from the model dropped; the echo translator's colours; the pipeline with a
coloured link and an ID (Done, Partial, a dropped ID through the Brackets retry);
ChatML neutralization; grammar strings; the sanitize table; same-language detection, scripts
and the target-language evidence on the E1 echo and same lines; `Nothing` and the ko shortcut;
the E1b pipeline paths (V2 insist once and its fallback, V3 evidence, V4 expansion in both
forms, bracket repair, at most one follow-up, tuning labels); cache key stability (golden value)
and LRU; the `policy::choose` table; flag parsing on both fixtures; the launch command line on
both fixtures and an old build; manifest discovery and the runtime search order in temp folders;
the job object (a child test process starts `ping -n 30` in a job and exits; ping is gone within
2 s); the stderr ring and key file deletion with a real child; argument quoting; the HTTP client
against an in-process stub (JSON, SSE stream with partials, 401, hang, dropped connection); NVML
against PDH on this PC; and the lifecycle state machine with a fake spawner and devices over the
HTTP stub: echo translator, CPU happy path (command line, key file, warm-up, body fields, cache,
`no_cache`), the default tuning (expanded message in the request, an echoed foreign line shown
as text, a French echo answered Same), placeholder retry, bad output and HTTP errors, GPU start
hang with CPU fallback, Vulkan marker, CPU start failure with the 60 s retry window, 3 crashes and
the Error hold, request timeout with lazy restart, 2 GPU failures, idle unload and `keep_loaded`,
WoW gone, VRAM guard and `set_backend`, queue overload, cancel, clear and merge, urgent jobs (ahead
of the waiting ones, in order, never dropped first by an overload, moving a shared entry forward),
`NotInstalled`,
`LowMemory`, shortcuts, and shutdown under 2 s with a hanging request. `evalprompt` tests parse
both corpora, check the expectation grammar, and check that every pair the corpora mark
must-use or keep-allowed is classified the same way by `prompt::keep_allowed`.

## 10. E1, E1b, E1c and E1d evaluations

### E1 (2026-10-16, P3)

40 synthetic lines with hand-written glossary pairs; rule "glossary meaning present in >= 80% of
lines with pairs". Placeholders: script 28/28, first-pass placeholders 25/25, item names 19/19,
glossary 5/17, same 5/5, 7 lines answered `Same` although they needed a translation. Brackets:
item names 10/19, glossary 6/17, 5 wrong `Same`. Placeholders was kept as the default (deviation
1). Those numbers used the old corpus and are not comparable with E1b.

### E1b (2026-10-17, P3b): glossary policy and translation quality

**Policy** (the user's intent): game abbreviations that players use as-is in every language may
stay as typed (the glossary row explains them); names with an official localized form and words
with a glossary meaning in the target language must use that meaning; a line that is not in the
target language must never be reported as `Same`. Section 6 has the engine's keep-allowed list.

**Corpus changes** (`tests\eval\chat_samples.tsv`, still 40 synthetic lines, 5 same lines):
- `pairs_json` is now the real addon output (`Glossary.Extract` run under fengari on each line):
  several old pairs could never come from the addon (hc, lol, mp, cheap for zh) and the real
  meanings differ (dps = "personnage qui inflige des dégâts" for fr). en02 keeps one user entry
  (mp = whisper).
- Five lines changed to exercise the policy: fr15 is now a German line with "Todesminen", and
  de01, es01, ru01, ko02 gained a chat word (thx, ty, brb, thx).
- `expect` now classifies every pair: `use:<term>=a|b` (must-use, one alternative must appear),
  `keep:<term>` (keep-allowed, no check); plus `name=` (official names, 6 lines) and `word=`
  (content words showing a real translation, 18 lines). 18 lines have pairs: 10 with must-use
  pairs, 15 with keep-allowed pairs.
- New `tests\eval\echo_samples.tsv`: 14 short lines made mostly of chat abbreviations in another
  language than the target (the E1 echo case), same format.

**Metrics** (`evalprompt`, numbers and ids only). The text checked is the shown text: the output,
or the source line when the answer is `Same`. Rule: script 100% (every line that needs a
translation, `Same` answers included), first-pass placeholders >= 90%, item names restored 100%,
must-use glossary >= 80% of the lines with must-use pairs, wrong `Same` = 0, same detected on
every same line, no line without a result; then no regression on the rest (official names,
content words, unchanged lines shown as translations, latency).

**Run conditions.** CPU only, 4 threads, below-normal priority, batches of 6 lines, one server per
batch under a 60 s watchdog (never fired in any run), one server at a time, each killed and seen
exited, `tasklist` empty of llama-server after every run. Model Qwen3-4B Q4_K_M (Ollama store),
dev runtime 0.7.0-dev. Outputs are deterministic (seed 42): two runs of the shipped tuning
compared line by line were identical, and a third gave the same metrics. Another agent's builds ran on the PC at the same time; mean
latency varied by about 10% between identical runs, so smaller latency differences are noise.

**Results, main corpus** (`tests\eval\out\e1b_<variant>.json`; `report.json` is the shipped
tuning). Item-name and script denominators vary because lines answered `Same` are judged on the
source line and have no restored names.

| Variant | Script | Items | First pass | Must-use | Names | Words | Same | Wrong Same | Echo shown | Retries insist/brackets | Calls | Mean/max ms | Rule |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| V0 (E1 prompt) | 33/35 | 19/19 | 24/24 | 5/10 | 1/6 | 12/18 | 5/5 | 6 | 0 | 0/0 | 39 | 1751/4293 | fail |
| V1 | 35/35 | 22/22 | 23/24 | 3/10 | 1/6 | 16/18 | 5/5 | 1 | 0 | 0/1 | 40 | 1907/3855 | fail |
| V0+V2 | 33/35 | 21/21 | 24/24 | 5/10 | 1/6 | 12/18 | 5/5 | 4 | 0 | 6/0 | 45 | 1850/3949 | fail |
| V0+V3 | 33/35 | 22/22 | 24/24 | 5/10 | 1/6 | 12/18 | 5/5 | 0 | 6 | 0/0 | 39 | 1692/4029 | fail |
| V0+V4 | 33/35 | 18/18 | 24/24 | 7/10 | 1/6 | 11/18 | 5/5 | 6 | 0 | 0/0 | 39 | 1618/3944 | fail |
| V1+V2 | 35/35 | 22/22 | 23/24 | 3/10 | 1/6 | 16/18 | 5/5 | 1 | 0 | 1/1 | 41 | 1981/3883 | fail |
| V1+V3 | 35/35 | 22/22 | 23/24 | 3/10 | 1/6 | 16/18 | 5/5 | 0 | 1 | 0/1 | 40 | 1937/3728 | fail |
| V1+V4 | 35/35 | 22/22 | 23/24 | 10/10 | 1/6 | 16/18 | 5/5 | 1 | 0 | 0/1 | 40 | 1904/3786 | fail |
| V1+V2+V3 | 35/35 | 22/22 | 23/24 | 3/10 | 1/6 | 16/18 | 5/5 | 0 | 1 | 1/1 | 41 | 2005/3916 | fail |
| **V1+V3+V4 (shipped)** | 35/35 | 22/22 | 23/24 | 10/10 | 1/6 | 16/18 | 5/5 | 0 | 1 | 0/1 | 40 | 2006/3893 | **pass** |
| V1+V2+V3+V4 | 35/35 | 22/22 | 23/24 | 10/10 | 1/6 | 16/18 | 5/5 | 0 | 1 | 1/1 | 41 | 1916/3834 | pass |
| V0+V2+V3+V4 | 33/35 | 22/22 | 24/24 | 8/10 | 1/6 | 11/18 | 5/5 | 0 | 3 | 6/0 | 45 | 1890/3887 | fail |
| V1+V3+V4, Brackets variant | 35/35 | 22/22 | n/a | 10/10 | 1/6 | 16/18 | 5/5 | 0 | 1 | 0/0 | 39 | 2303/4334 | (not a ship candidate) |

"Echo shown" = a line that needed a translation, answered `Done` with its own text (V3 shows the
output instead of saying `Same`). The final run of the shipped tuning (`report.json`) gave the
same metrics (2080/4080 ms, 98.3 s for 40 lines).

Failing ids: V0 script zh01 ko02, must-use fr07 de01 es01 ru01 ko02, wrong Same fr07 fr13 en09
de01 zh01 ko02. V1 must-use fr07 fr10 fr12 de01 es01 ru01 ko02 (the casual prompt keeps chat
words such as brb, gz, pls, thx, ty), wrong Same en09. Shipped: names fr01 fr03 fr15 en01 en04,
content words fr09 en09, echo shown en09, first-pass placeholders en07 (the Brackets retry plus
the bracket repair restored its name).

**Results, echo set** (`tests\eval\out\echo_<variant>.json`): V1+V3+V4 and V1+V2+V3+V4 both give
script 14/14, must-use 10/12 (ec01 "omw" became "je suis là", ec09 "be right back" became "schon
in einer Minute"), content words 10/10, wrong Same 0, echoes 0, insist retries 0, 14 calls.

**Decision: `Tuning::DEFAULT` = V1+V3+V4** (casual prompt, `Same` only with evidence, must-use
terms expanded before the call). Each switch has a measured job: V1 alone cuts wrong `Same` from
6 to 1 and fixes the script failures (33/35 to 35/35) and content words (12 to 16 of 18), but
keeps chat words; V4 makes the must-use meanings reach the output (3/10 to 10/10 with V1); V3
removes the last wrong `Same` (en09). V1+V2+V3+V4 also passes with identical results on both
sets; V2 fired once (en09, the echo came back again) and never on the echo set, so it is left
off: same quality, one model call less. V2 stays available as a switch.

**What the rule does not show** (from the console outputs of the shipped tuning, synthetic
lines, not stored):
- Official names: 1/6. The model keeps or translates literally deadmines, Stormwind, Todesminen
  ("mines de mort"), Mortemines and Masacre ("slaughter"); only Ironforge became 铁炉堡. The
  "use the official names" line is not enough for a 4B model.
- Model errors kept: en02 "pas cher" became "expensive" (meaning inverted), fr16 invented
  "cherément", ru01 "warrior tank" became a wrong compound, fr08 "i will tip" mistranslated, fr09
  "dungeon" left in English, de04 wrong article.
- Latency: mean per model line about 1.9 to 2.1 s on CPU against 1.75 s for V0 (longer, more
  natural outputs and expanded text), within the CPU deadline (6 s + n_predict/8 s).

**Follow-ups (not done here):**
1. Official names belong in the addon glossary: full names as must-use entries (deadmines = Les
   Mortemines, stormwind = Hurlevent, and German or Spanish names such as todesminen or masacre
   with their target meanings). V4 then puts the official name into the message before the call,
   which is the mechanism that made must-use reach 10/10. Addon files are outside P3b.
2. GPU pass: not run, WoW was running (amendment A1).

### E1c (2026-10-19, user request L4): tokens the model must copy

**In-game report.** A Chinese yell ending with "加V <ID>或<ID>" (192 bytes, two IDs of letters
and digits) came back in French ending with "Ajoutez V": the IDs were gone, although the Helper
received the whole line.

**Corpus** (`tests\eval\chat_samples.tsv`, now 54 synthetic lines, synthetic IDs and addresses
only): 14 lines with IDs, URLs and e-mail addresses (fr17 to fr21, en11 to en15, de05 to de07,
es04), among them two long Chinese yells shaped like the in-game one (fr20, 71 characters; fr21)
and one line with a coloured link (de07, `\4FFFFFF`). New expectation `copy=<token>`: the shown
text must hold the token exactly as written (metric `verbatimKept`, rule 100%). The 40 older
lines are unchanged, and the new lines carry no must-use pair, no `name=` and no `word=`, so the
older metrics keep their denominators. Pairs come from the real `Glossary.Extract` (en11 gets
`gg` from `discord.gg`, de05 `org` from `example.org`, both keep-allowed).

**Run conditions** as for E1b (CPU, 4 threads, batches of 6, one server per batch under a 60 s
watchdog that never fired, one server at a time, each killed and seen exited, no llama-server
left after any run). WoW was running and the PC busier than for E1b: mean latency about 2.5 s per
line against 2.0 s for the same lines in E1b (identical outputs), so latency differences are
noise here.

| Run (main corpus, shipped tuning v1+v3+v4) | Script | Items | First pass | Must-use | Verbatim | Wrong Same | Echo shown | Brackets retries | Calls | Mean/max ms | Rule |
|---|---|---|---|---|---|---|---|---|---|---|---|
| HEAD 1332f01, 40 lines (reproduction) | 35/35 | 22/22 | 23/24 | 10/10 | n/a | 0 | 1 | 1 | 40 | 2530/4605 | pass |
| HEAD 1332f01, 54 lines (`out\e1c_before.json`) | 49/49 | 23/23 | 24/25 | 10/10 | 13/14 | 0 | 1 | 1 | 54 | 2612/9085 | fail |
| This change, 54 lines (`out\report.json`) | 49/49 | 23/23 | 37/38 | 10/10 | **14/14** | 0 | 1 | 1 | 54 | 2581/8060 | **pass** |

- HEAD reproduced the E1b report line for line (outcomes, spans, every check; only times differ).
- With this change the 40 older lines and the 14-line echo set give exactly the same per-line
  results as before (no token found in them, same prompts, same outputs); the 14 new lines all
  pass their placeholders on the first try.
- Failing ids, unchanged: names fr01 fr03 fr15 en01 en04, words fr09 en09, echo shown en09, first
  pass en07. HEAD's verbatim failure: fr20.

**Cause of the in-game drop: the output length cap, not the model.** A probe of six synthetic
Chinese yells ending with two IDs (console only, not stored):
- HEAD: 5/6 kept. The 71-character yell (fr20's text) came back cut at exactly 253 characters
  (3 x 71 + 40) before its end, the IDs with it; the 44-character one was cut at exactly 172 (its
  IDs came earlier and survived). A Han character comes out as several Latin characters, so the
  old cap (3 x input characters + 40) cut French translations of long Chinese lines.
- Protection with the old cap: still 5/6. The 71-character yell ended "... peuvent ajouter V",
  the in-game symptom: the cut removed the placeholders, the Brackets retry was cut the same way,
  and the IDs were lost.
- This change (protection and a wide character counting 3 in the cap): 6/6, the long yell
  complete with both IDs.

So both parts ship: the cap (deviation 26) removes the cause, and the protection (section 6, step
4) keeps the tokens exact in every other case (the model may otherwise respell or translate an
ID, a URL or an address). Protected tokens also leave the letters they hold out of the `Nothing`
and `ko` shortcuts and of the same-language evidence.

**Integration review (2026-10-19).** The review found levels, prices, raid sizes, times and ratings
in other languages' shorthand protected as IDs (`niv60`, `60лвл`, `5g50`, `50кк`, `10hm`, `20Uhr`,
`pvp1800`...), chat words joined by a dot protected as domains (`add.me`, `merci.de`, `lol.gg`) and
full-width IDs never protected (deviation 27). Five synthetic lines were added (corpus 59 lines):
fr22 (a Russian level `60лвл`), fr23 (a price `2g50`), en16 (a French level `niv60`), de08 (a time
`20heures` and a raid size `icc10hm`), each with a `word=` check and the new `free=` expectation
(the token must not be a placeholder, checked on the engine's own protection), and en17 (a
full-width ID, `copy=`). Same run conditions (CPU, 4 threads, batches of 6, 60 s watchdog never
fired, 10 servers killed and seen exited per run, at most one llama-server at any second of a
per-second process count, none left; WoW running).

| Run (59 lines, shipped tuning v1+v3+v4) | Script | Items | First pass | Must-use | Verbatim | Free | Wrong Same | Same | Words | Calls | Mean/max ms | Rule |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Before the review fixes (`out\e1c_review_before.json`) | 54/54 | 23/23 | 41/42 | 10/10 | 14/15 (en17) | 0/4 | 0 | 5/5 | 21/23 | 59 | 2525/7255 | fail |
| After (`out\report.json`) | 54/54 | 23/23 | 38/39 | 10/10 | **15/15** | **4/4** | 0 | 5/5 | 21/23 | 59 | 2668/7876 | **pass** |

- Only the five new lines changed; the 54 older lines give identical per-line results (outcome,
  spans, first pass, every check). The first-pass denominator fell by 3: fr22, fr23 and en16 have
  no placeholder any more, en17 gained one (and de08 lost its two).
- Failing ids, unchanged from E1c: names fr01 fr03 fr15 en01 en04, words fr09 en09, echo shown
  en09, first pass en07.
- Unit tests hold the shapes the probe found (`verbatim::tests::other_languages_shorthand_stays_plain`,
  `chat_words_joined_by_a_dot_are_not_domains`, `full_width_ids_and_urls`).

### E1d (2026-10-20, integration of the compact bubbles): output lengths

The addon's bubble row factor for Latin to Latin (1.25, `docs\ADDON.md` decision 79) needed output
lengths, which no report kept. `evalprompt` now records, for every `done` line, the characters of
the source and of the shown output outside the protected spans (item names, `{...}` groups, IDs,
URLs; spaces collapsed) and their ratio, with the script pair (`pair`, `inChars`, `outChars`,
`lengthRatio` per line; `lengths`: lines, mean, median, P75, P90 and max per pair, and per pair
for the lines of 30 characters or more). Counts only: no text reaches the report.
`out\e1d_lengths.json`: the shipped tuning, CPU, 4 threads, 10 servers each killed and seen
exited, the watchdog never fired, none left, 168 s; every gate passes with the per-line results
of `out\report.json` (script 54/54, items 23/23, must-use 10/10, verbatim 15/15, free 4/4, wrong
Same 0).

| Pair | Lines | Median | P75 | P90 | Max |
|---|---|---|---|---|---|
| Latin to Latin | 37 | 1.167 | 1.308 | 1.778 | 2.5 |
| Latin to Latin, 30 characters or more | 22 | 1.063 | 1.297 | 1.387 | 1.421 |
| Cyrillic to Latin | 4 | 1.273 | 1.351 | 1.48 | 1.48 |
| Han to Latin | 5 | 4.417 | 4.926 | 4.962 | 4.962 |

The short Latin lines run higher (an abbreviation spelled out by the must-use glossary) but give
35 characters out at most, one bubble row. The other pairs have 1 to 5 lines each: too few to
change their factors.

## 11. Deviations from the design and decisions

1. **E1 decision** (section 10): Placeholders kept although the literal ship rule said Brackets.
2. **NVML matching:** NVML exposes no LUID, so the device is matched by PCI device id (device << 16
   | vendor) and subsystem id; with two identical cards (ambiguous) the PDH path is used.
3. **Server ring in the log:** `EngineEvent::ServerOutput { line: ServerLine }`, a newtype only the
   allowlist filter can build (this settles the M0 open point); the filter also refuses lines that
   could echo a prompt or a completion.
4. **Handle inheritance:** `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` limits inheritance to NUL and the
   output pipe (std's `Command` cannot create a suspended process whose thread can be resumed).
5. **E1 watchdog:** 40 lines take about 80 s on CPU, longer than one 60 s watchdog, so each batch
   of 6 lines runs on its own server under its own 60 s watchdog.
6. **Echo translator:** uses the queue, cache and shortcuts, so E2E behaves like the real engine.
7. **Unknown target code:** English language name and the Latin grammar (cannot come from the
   addon).
8. **Typed `{N}`:** rewritten `{ N }`; **Brackets retry:** spans not already in brackets are
   wrapped and unwrapped.
9. **CUDA folder:** `cuda_v12` before `cuda_v13`.
10. **Explicit `--model`:** GGUF magic only; the 2.3-2.7 GB window applies to manifest discovery.
11. **`JobDone` outcomes** add `"not_installed"`; events `DiscoveryRejected`, `UnknownModel`,
    `FlagsProbeFailed`, `ServerOutput` and `GpuBlocked` were added; `Backend` and `ServerLine` are
    exported.
12. **Building blocks** (`client`, `device`, `discover`, `flags`, `grammar`, `model`, `pipeline`,
    `policy`, `post`, `protect`, `server`) are `#[doc(hidden)] pub` for `evalprompt`; they are not
    part of the D.2 interface.
13. **E1b: a language line in the prompt.** Design D.5 says the language is enforced only by the
    grammar, because a "translate into X only" line broke glossary use. E1b confirms the effect
    (the casual prompt alone: must-use 3/10 against 5/10) but also measures that with must-use
    terms expanded before the call (V4) the glossary reaches 10/10 and the casual prompt removes
    most echoes; the shipped prompt therefore has "Reply with the {Language} translation only."
14. **E1b: glossary policy in the engine.** `prompt::keep_allowed` is a copy of the game
    abbreviations of the addon's built-in glossary (plus tank, heal, healer, dd, hc, lol); the
    TRANSLATE message carries no per-pair flag, so the protocol is unchanged. A new addon
    abbreviation counts as must-use until it is added to the list.
15. **E1b: V2 trigger.** "Input has glossary pairs" is read as "has must-use pairs": keep-allowed
    terms may legitimately stay, so an echo of them is not a failure. The second trigger ("script
    or language not the target") uses the same evidence check as V3.
16. **E1b: E1 rule and corpus replaced.** "Glossary meaning present >= 80% of lines with pairs"
    became "must-use meaning present >= 80% of lines with must-use pairs", wrong `Same` = 0 was
    added, and the pairs became the real addon output (section 10). Fact: several hand-written
    pairs could not come from the addon.
17. **E1b: bracket repair.** Fact: in E1b the Brackets retry of en07 translated a Korean item name,
    breaking the item-name rule; Brackets answers now get their item-name segments back when the
    segment count matches.
18. **E1b: V4 in the Brackets form.** Fact: with expansion limited to the placeholder form, the
    Brackets variant kept must-use meanings in 4/10 lines; expanding outside the brackets gives
    10/10.
19. **`PROMPT_VERSION` 2** (prompt text changed); the cache key golden value changed with it.
    Additions to the prompt module: `Style`, `DEFAULT_STYLE`, `Opts`, `build_with`,
    `system_prefix_with`, `keep_allowed`, `must_use`, `single_meaning`; `build`, `language_name`,
    `Variant` and `DEFAULT` keep their D.2 signatures. New event `EngineEvent::InsistRetry`
    (V2 only); the selftest JSON gains `engine.tuning`.
20. **`evalprompt`:** `--tuning`, `--show-raw` (raw answers of every attempt, console only); a rule
    with nothing to judge passes (the echo set has no item names).
21. **Selftest discovery injectable (review fix).** The unit test `selftest_shape_without_runtime`
    gave missing runtime and model paths, but discovery falls back to the Ollama install under
    `%LOCALAPPDATA%`, so every plain `cargo test` (and `scripts\helper.ps1 -Test`) ran
    `llama-server --help` on a machine with Ollama installed, against the rule that only the
    engine and integration steps start llama-server. `selftest::run_with` takes the discovery; the
    unit test passes an empty one, and the real-machine variant `selftest_on_this_machine` is
    `#[ignore]`.

22. **Urgent jobs (2026-10-19, conversation window, design B.3 H9).** `Job` gained `urgent`, and
    `worker.rs` passes it to the queue entry (`Entry::urgent`); that one line in `worker.rs` is
    outside the files the work package listed (`lib.rs`, `queue.rs`), because the worker builds
    the entries.
23. **Link colours (2026-10-19, L3).** The mark grammar moved to `tp_bridge::marks` (shared with
    the app), with the colour prefix `0x04 RRGGBB` the addon writes right after a `0x02` since
    build 5. `Protected` gained `colors` and `verbatim` (per span) and `span_key`; `restore` writes
    a coloured span back as a coloured mark, so `Done` and `Partial` texts can hold marks
    (section 1). The alternative, a separate list of runs in `Status`, would have changed the D.2
    interface and the queue's merge semantics for one more field; the marks keep one `ChatText`.
24. **Cache key holds the spans (review fix, 2026-10-19).** Fact: the key was the protected text
    and pairs only, so "wts [Linen Cloth]" and "wts [Silk Cloth]" (both "wts {1}") shared one
    cache entry and one queue merge, and the second line was shown with the first one's item
    name. With IDs as placeholders (L4) and colours (L3) this would happen for every "add {1}".
    The key now includes `Protected::span_key` (contents and colours, length-prefixed);
    `PROMPT_VERSION` went to 3 and the golden key with it (`cache.rs` test).
25. **Verbatim tokens (2026-10-19, L4)** are found by the engine, not marked by the addon: the
    addon's marks are for real links, and the engine already owns the prompt's protection. A
    handle with `_` is any run holding a `_` and 2 alphanumerics, so the neutralized ChatML token
    `< |im_end| >` now carries `im_end` as a placeholder (harmless: it is restored as written;
    `pipeline.rs` test). `MAX_SPANS` stays 9: one-digit placeholders are the form E1 and E1b
    measured (two-digit ones were never measured), a chat line rarely holds 9 links and IDs, and
    when it does the marked links keep priority (`protect::tests::marks_keep_priority_over_tokens`).
26. **Length cap weighted for wide scripts (2026-10-19, L4, fact in E1c).** The user's report was a
    dropped pair of IDs; the measured cause is the output cap (3 x input characters + 40), which
    cut the French translation of a long Chinese line before its last words. Protecting the IDs
    alone would not have saved them: their placeholders would have been cut too. A Han, kana,
    Hangul or full-width character now counts 3; runaway answers stay bounded by the cap and by
    n_predict.
27. **Token shapes after the integration review (2026-10-19, facts in E1c).** The first L4 shapes
    knew only English and French game numbers: a review probe found `niv60`, `лвл60`, `60лвл`,
    `5g50`, `5po50pa`, `50кк`, `10hm`, `20Uhr`, `pvp1800` protected as IDs (and E1 showed `60лвл`,
    `50кк`, `niv60` and `2g50` copied untranslated), `add.me`, `merci.de` and `lol.gg` protected as
    domains, and a full-width ID never protected (it came back respelled "KIWI48213"). The shapes
    now cover the other languages' level, amount, time and rating shorthand by word lists (the
    alternative, any 1-3 letters with 1-3 digits as a game number, would also have released short
    handles such as `Bob22`, which the request names as IDs), bare domains need a known host, a
    first label of 4 characters or 3 labels, and full-width letters, digits and punctuation are
    read as ASCII. New E1 expectation `free=<token>`: a token the engine must not protect, checked
    on the engine's own protection without the model (metric `freeTokens`, rule 100%).
28. **Remote services (2026-10-20, section 13): the fallback reason is a field of `Update`**
    (`fallback: Option<Fallback>`), not a new `Status` variant: `Status`, the queue's `St` and
    the merge semantics stay as they were, and a job that fell back still ends with the status
    the local path gives it (`Done`, `Same`, `Failed`, `LowMemory`, `NotInstalled`). The only
    other interface changes are `EngineHandle::set_remote`, the `remote` module and four log
    events (`RemoteConfigured`, `RemoteRequest`, `RemoteDone`, `RemoteFallback`).
29. **The prompt is split for the remote LLMs:** `prompt::system_text` and `prompt::user_text`
    are the two halves of `build_with` (ChatML wrapper unchanged, goldens unchanged,
    `PROMPT_VERSION` still 3); `Prepared` gained `system`, `user` and `listed` (the pairs the
    prompt lists).
30. **WinHTTP total timeout enforced by the caller** (fact in section 13): each request runs on
    its own thread and the caller waits at most the total.
31. **Review fixes (2026-10-21, section 13):** keys are stored per service id, not per provider
    (`keys.bin` version 2); a new `FallbackKind::KeptLocal` (`kept_local`) for the lines whose
    URLs, e-mail addresses or IDs would otherwise reach a service in clear; `Protected` gained
    `tokens_in_clear`; the daily cap counts the answer too and only what a service answered for.

## 12. M0 skeleton notes (kept)

- The root types (`JobKey`, `Job`, `FailKind`, `Status`, `Update`) live in `lib.rs`;
  `EngineConfig`, `BackendPref` and `Translator` in `config.rs`; `EngineEvent` and `EngineState`
  in `events.rs`; `Engine` and `EngineHandle` in `worker.rs`. All are re-exported at the crate
  root.
- `prompt::Variant` derives `Clone, Copy, Debug, PartialEq, Eq`.

## 13. Remote translation services (2026-10-20, user request)

**Request and decisions.** The user asked whether players can enter their own API keys for remote
AI services, for a better or faster model. Decisions (user and lead, 2026-10-20): the providers
of the first version are DeepL, any OpenAI-compatible endpoint, Google Gemini and Anthropic
Claude; the local Qwen model stays the default; keys are entered only in Translate Companion
(never in WoW) and stored encrypted with DPAPI; when the service fails (network, quota, invalid
key, refusal, timeout) or the daily character cap is reached, the job is translated locally and
its result carries the reason, which the companion shows as a warning badge with a tooltip.
Module: `helper\engine\src\remote\` (`mod.rs`, `http.rs`, `winhttp.rs`, `deepl.rs`, `openai.rs`,
`gemini.rs`, `claude.rs`, `keystore.rs`, `usage.rs`; tests in `tests.rs` and `fake.rs`).

### Interface for the app

```rust
use tp_engine::remote::{self, keystore, RemoteConfig, RemoteSettings, Secret};
let keys_path = keystore::path_next_to(&settings_json_path);       // %APPDATA%\TranslateProbe\keys.bin
let keys = keystore::load(&keys_path)?;                             // missing file: no keys
match RemoteConfig::from_settings(&remote_settings, &keys) {        // consent, key, URL, model checked
    Ok(cfg) => h.set_remote(cfg),                                   // Ok(None) for "local"
    Err(e) => { /* e.code(): no_consent, no_key, bad_url, insecure_url, model_required, bad_model */ }
}
keystore::save(&keys_path, &keys)?;                                 // Keys::set(&service_id, Secret::new(..))
let cfg = RemoteConfig::new(provider, model, base_url, Secret::new(key), cap)?;   // the Test button
let r = remote::test_translation(&cfg, target, &*remote::system_transport());     // blocking, <= 15 s
let used_today = remote::usage::read_today(&data_dir);
```

- `RemoteSettings` (serde, camelCase, for settings.json): `provider` (`local`, `deepl`, `openai`,
  `gemini`, `claude`), `model` (empty: the provider's default), `baseUrl` (OpenAI-compatible
  only, empty: `https://api.openai.com/v1`), `consent`, `dailyCharCap` (0 = none). `validate()`
  fixes invalid values like the app's own settings.
- Service id (`remote::service_id(provider, base_url)`, `RemoteSettings::service_id()`,
  `RemoteConfig::service_id()`): `deepl`, `gemini`, `claude`, or for an OpenAI-compatible server
  `openai:<host[:port]><base path>` (`openai:api.openai.com/v1`; the query, a trailing `/` and
  `/chat/completions` are left out, so one server written two ways is one service). It names
  both the consent and the saved key. Consent: `consent` holds the service id the player agreed
  to send chat text and glossary terms to; `give_consent()` records it, `has_consent()` checks
  it. Another provider, server or route behind the same host needs a new consent.
- Keys: `keystore::Keys` maps a service id to a `Secret` (`get(id)`, `set(id, key)`,
  `remove(id)`, `slots()`). `from_settings` takes the key saved for the settings' very service
  id, so a key saved for one OpenAI-compatible server is never sent to another one, nor to a
  keyless server on this PC. `remote::key_needed(provider, base_url)` says whether a key is
  required (every service but an OpenAI-compatible server on this PC or a private address).
- `Provider`: `id()`, `from_id()`, `display_name()` (product names; the app localizes the
  "OpenAI-compatible" label), `default_model()`, `model_suggestions()` (the first is the
  default), `uses_base_url()`.
- `Fallback` (on `Update::fallback`): `provider`, `kind` (`FallbackKind`: `Http`, `Timeout`,
  `Network`, `Refused`, `Truncated`, `BadAnswer`, `DailyCap`, `KeptLocal`; `code()` gives `http`,
  `timeout`, `network`, `refused`, `truncated`, `bad_answer`, `daily_cap`, `kept_local` for the
  locale tables), `status` (HTTP status when the service answered with an error) and `message`
  (the service's own short message, English as the service wrote it, sanitized: no key, at most
  160 characters, may be empty; for `DailyCap` the cap in force, in decimal digits, so an old
  badge shows the limit that applied). Its Debug prints provider, kind and status only.
- `remote::test_translation` sends `TEST_SENTENCE` (synthetic, no player data) through the same
  pre- and post-processing; `Err` is the `Fallback` the badge would show.

### Providers

Formats checked on the providers' documentation on 2026-10-20 (DeepL, Gemini and OpenAI with
WebFetch; Claude from Anthropic's API reference given by the lead). Every request carries only
the protected message text (placeholders for item links, IDs and URLs, as for the local model)
and the glossary pairs; never a sender name, a channel or a realm.

| Provider | Request | Model (default, suggestions) | Answer |
|---|---|---|---|
| DeepL | `POST https://api.deepl.com/v2/translate` (key ending in `:fx`: `api-free.deepl.com`), `Authorization: DeepL-Auth-Key <key>`; `text: [xml]`, `target_lang`, `context`, `tag_handling: xml`, `ignore_tags: ["x"]`, `preserve_formatting: true`, `formality: prefer_less`, `model_type` when set | DeepL's own default; `latency_optimized`, `quality_optimized`, `prefer_quality_optimized` | `translations[0].text` |
| OpenAI-compatible | `POST {base}/chat/completions` (`/chat/completions` appended unless the URL ends with it), `Authorization: Bearer <key>` (none without a key); `model`, `messages: [system, user]`, `stream: false`, an output limit of 4000 tokens (`max_completion_tokens` on `api.openai.com`, `max_tokens` elsewhere); `reasoning_effort` only on `api.openai.com` (`none` for gpt-6-luna, `low` for gpt-5/gpt-6/o-series) | `gpt-6-luna` on OpenAI's host, required elsewhere; `gpt-6-luna`, `gpt-6.1-sol`, `gpt-6-astra` | `choices[0].message.content`; `message.refusal` or `finish_reason: content_filter` refused, `length` truncated |
| Google Gemini | `POST https://generativelanguage.googleapis.com/v1beta/models/<model>:generateContent`, `x-goog-api-key` (never `?key=`); `systemInstruction`, `contents[user]`, `generationConfig: {maxOutputTokens: 4000, thinkingConfig}`, safety thresholds `BLOCK_ONLY_HIGH` | `gemini-3.5-flash-lite`; `gemini-3.8-flash`, `gemini-3.1-flash-lite` | `candidates[0].content.parts[]` without `thought: true`; `MAX_TOKENS` and `CONTINUATION` truncated; `SAFETY`, `RECITATION`, `LANGUAGE`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `IMAGE_*`, `ESCALATION`, `PUP_LIMITED_DISABLED` or `promptFeedback.blockReason` refused; any other reason (`OTHER`, `MALFORMED_RESPONSE`, `MISSING_THOUGHT_SIGNATURE`, the tool-call reasons) an unusable answer with the reason's name |
| Anthropic Claude | `POST https://api.anthropic.com/v1/messages`, `x-api-key`, `anthropic-version: 2023-06-01`; `model`, `max_tokens: 4000`, `system`, `messages: [user]`, `output_config: {effort: low}`; for claude-opus-5-5, claude-sonnet-5-5, claude-opus-5 and claude-fable-5-1 also `anthropic-beta: server-side-fallback-2026-07-01` and `fallbacks: "default"` | `claude-opus-5-5`; `claude-sonnet-5-5`, `claude-haiku-5-5` | `stop_reason` first (`refusal` refused, `max_tokens` truncated), then the `text` blocks only |

- LLM providers (OpenAI-compatible, Gemini, Claude) get the local prompt split in two:
  `prompt::system_text` (the casual system prompt) and `prompt::user_text` (`Glossary:`, then
  `Message:` and the text), the same tuning (V1+V3+V4: must-use terms expanded into the message).
- Not sent, on purpose: `temperature`, `top_p`, `top_k` (removed or limited to the default on the
  current Claude and OpenAI reasoning models; the Gemini 3 guidance keeps the default), a
  `thinking` parameter for Claude (Opus 5.5 cannot disable thinking; effort `low` keeps it
  short), an assistant prefill (a 400 on current Claude models). Every LLM provider gets an
  output limit of 4000 tokens (reasoning included), so a chat line written to make the model
  ramble cannot run up the player's bill. Claude's `output_config.effort` is
  sent only to models that take it (the 4.6 generation and later; Haiku 4.5 and older refuse it).
- Gemini thinking (the thinking guide's table, read again 2026-10-21): `thinkingLevel` only for
  the models it lists, `minimal` where supported (3.6 Flash, 3.5 Flash-Lite, 3 Flash, 3.1
  Flash-Lite Image), `low` for the others (4 Argon, 3.8 Flash, 3.1 Pro, 3 Pro, 2.5 Pro, 2.5
  Flash). Any other model (3.1 Flash-Lite, 2.5 Flash-Lite, a typed name) gets no
  `thinkingConfig`: its default applies, and an undocumented level cannot turn every request
  into a 400.
- DeepL placeholders: the text is XML-escaped and each `{N}` (in the Brackets form each
  `[...]` segment) is wrapped in an ignored `<x>` tag, which DeepL never translates; in the
  answer the `<x>` tags are removed first (DeepL may move or duplicate them), then the entities
  are decoded, so text a player typed as `<x>` survives. `context` is "World of Warcraft chat."
  plus the glossary pairs the prompt would list (not translated, not billed). Target codes: en
  `EN-US`, fr `FR`, de `DE`, es `ES`, it `IT`, pt `PT-BR`, ru `RU`, ko `KO`, zh `ZH-HANS`,
  zh-TW `ZH-HANT` (`EN`, `PT`, `ZH` alone are the unspecified variants; DeepL's language list
  marks `en` as not usable as a target).
- OpenAI-compatible servers: works with OpenAI, OpenRouter (`https://openrouter.ai/api/v1`),
  Mistral (`https://api.mistral.ai/v1`), Groq (`https://api.groq.com/openai/v1`), DeepSeek
  (`https://api.deepseek.com`), LM Studio (`http://localhost:1234/v1`) and Ollama
  (`http://localhost:11434/v1`). No key is needed for a loopback host (`localhost` itself,
  127.0.0.0/8 and `::1` literals) or a private network address literal (10/8, 172.16/12,
  192.168/16, 169.254/16, fc00::/7, fe80::/10: Ollama on a NAS); plain `http://` with a key is
  refused for any host but a loopback one (`insecure_url`: the key would travel in clear). Other
  names, `*.localhost` included, are not loopback: Windows sends them to the DNS server, whose
  answer may point anywhere. A server other than OpenAI's needs a model name.

### Transport, retry and timeouts

- `Transport` (`http.rs`) is one POST: URL, headers, JSON body; status, `retry-after`, body
  (1 MiB at most). `WinHttp` (`winhttp.rs`, windows crate feature `Win32_Networking_WinHttp`):
  TLS by the OS (1.2 and 1.3 only), the system proxy settings
  (`WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`; a loopback host or a private network address literal
  goes direct), redirects never followed
  (they would carry the key to another host), no cookies, no automatic authentication, one
  session per proxy mode kept for connection reuse.
- Timeouts: connect 5 s, total 15 s (`remote::Policy`). Fact (2026-10-20, loopback stub that
  reads the request and never answers): synchronous `WinHttpSendRequest` returned only after
  about 4 s with every WinHTTP timeout at 600 ms, and only when the server answered with 5 s
  timeouts. So each WinHTTP request runs on its own short thread and the caller waits at most
  the time left of the total; a late request ends on its thread and its result is dropped. The
  worker keeps a backstop deadline 2 s after the total.
- One quick retry for 408, 429, 500, 502, 503, 504 and 529, after `retry-after` (else 500 ms),
  only when at least 3 s of the total are left after the wait; not for an OpenAI 429 with code
  `insufficient_quota` or a DeepL 456 (quota). A timeout is not retried.
- Error messages come from the JSON error body (`error.message`, `message`, `error` as a string,
  `detail`); an HTML error page gives an empty message. `sanitize_message` replaces any word that
  holds a 6-character piece of the key, a masked key (`**`, `...`), a key-like prefix (`sk-`,
  `AIza`) or a run of 24 or more letters, digits, `-` and `_` by `[redacted]` (except the
  configured model name, which a 404 names, and an all-caps enum name such as
  `MISSING_THOUGHT_SIGNATURE`; a word holding a piece of the key is always redacted), collapses
  whitespace and control characters and cuts at 160 characters with "...".

### Routing in the worker

- The route of a job is chosen when it becomes the job in flight: the remote service when one is
  set (the echo translator never uses it), else the local model. A job already in flight keeps
  its route when `set_remote` changes the service.
- Remote: `pipeline::prepare` (shortcuts `Nothing` and `Same` answered without a request), the
  daily cap check, the request on a `tp-engine-remote` thread, then `pipeline::finish` and
  `pipeline::next` as for the local model: a broken placeholder gives one Brackets retry at the
  same service, a second failure is a `BadAnswer` fallback.
- What leaves the PC: the system prompt, the glossary pairs and the protected message text, in
  which item links, URLs, e-mail addresses and IDs are placeholders. Two cases would put a
  verbatim token in clear, and both stay local with the reason `kept_local` instead: a line with
  more links and tokens than the 9 placeholders (`Protected::tokens_in_clear` > 0; nothing is
  sent), and a Brackets retry of a line holding a URL, an e-mail address or an ID (that form
  writes them inline; the first request was sent with placeholders, the retry is not). A line
  with item links only still gets its Brackets retry at the service (item names are chat text).
  Names a player typed inside the message are part of its text and are sent.
- Fallback: the reason is kept for the job, the local path serves it (server start, `Loading`,
  `Partial`...), and every later update carries `fallback`; when the local path cannot run, the
  final status is the existing `LowMemory`, `NotInstalled` or `Failed` with the reason. A local
  answer to a job that fell back is not cached, so the next request tries the service again.
- With a remote service set, a missing local model does not answer `NotInstalled` at submit,
  and a local `LowMemory` does not fail the waiting jobs (they may still go to the service).
- Daily cap: per local day, the characters of the message text and of the glossary pairs sent
  (the fixed instructions are not counted) plus the characters of the answer. Checked before each
  request (the Brackets retry included) with the characters to send; counted when the service
  answers: a usable answer counts what was sent and received, a refused, truncated or unusable
  2xx answer what was sent (it was billed), an HTTP error, a network error or a timeout nothing
  (so an invalid key keeps showing its real error, never the daily limit). Past the cap nothing
  is sent and the job falls back with `daily_cap`, its message holding the cap in force. Stored
  in `<data dir>\remote_usage.json` as `{"version":1,"day":"YYYY-MM-DD","chars":N}`.

### Keys and privacy

- `keys.bin` next to settings.json: the 4 bytes `TPK1`, then a DPAPI blob
  (`CryptProtectData`, current user, `CRYPTPROTECT_UI_FORBIDDEN`, an application entropy) of
  `{"version":2,"keys":{"<service id>":"<key>"}}` (version 1 was keyed by provider and never
  shipped; its `openai` entry is dropped at load). DPAPI protects the file against other Windows
  users, other computers and a copy taken elsewhere; the entropy is a public constant, so it does
  not keep out programs running as the same user. The clear JSON exists only in memory and its
  strings are zeroed after use (best effort: serde_json's parsed and built values, and one buffer
  large enough not to grow); an empty key set deletes the file; a damaged or foreign blob is an error
  (`KeyStoreError::Corrupt` or `Decrypt`), never a crash. The companion writes it from its own
  process, so the MSIX redirection of the development shell does not apply at run time; tests
  use temporary folders only.
- `Secret` (the key in memory): no Debug (prints `Secret(..)`), no Display, no Serialize, zeroed
  on drop. `RemoteConfig`, `HttpRequest` and `HttpResponse` have no Debug; `HttpRequest` zeroes
  its header values (one holds the key) when dropped, and the WinHTTP header buffers are zeroed
  after sending. This is best effort: a copy left behind while a string grew is not reached.
- Log events carry the provider id, a model label (a known suggestion, `default` or `custom`;
  never a typed model name), HTTP status, latency, attempts and character counts only. The tests
  check, with a sentinel key and a sentinel chat word, that neither reaches a log event or a
  fallback message.

### Tests (all on the fake transport or a 127.0.0.1 stub)

Request shape per provider (URL, headers, body fields present and absent, free and paid DeepL
hosts, OpenAI-compatible bases for OpenRouter, Mistral, Groq, DeepSeek and a keyless Ollama,
Gemini thinking table, Claude effort and fallbacks per model), answers through the shared
post-processing with a coloured link and an ID restored, the Brackets follow-up, every HTTP error
class per provider (400, 401, 403, 404, 413, DeepL 456 not retried; 429, 500, 503, 529 retried
once; an HTML error page), the retry budget and quota errors, timeouts and network errors,
refusals and truncations per provider, message sanitizing, settings JSON and consent per
service, `from_settings`, the Test button, the cap count; the DPAPI key store round trip on a
temp folder (no key in clear on disk in UTF-8 or UTF-16, damaged and foreign blobs refused); the
usage file; WinHTTP against a loopback stub (headers and body, `retry-after`, a redirect not
followed, the total timeout with a server that never answers, a refused connection); and the
worker: a remote job that never starts llama-server and is cached under the remote identity, a
401 that falls back with the reason on every later update and the key redacted, the daily cap,
the cache key per provider and model, the remote Brackets retry, a remote timeout, a service
without a local model (`NotInstalled` and a refusal with the reason), the echo translator
ignoring the service, and no key or chat word in any log event. Since the review of 2026-10-21
also: DeepL answers with an entity before CJK, Hangul or Cyrillic text (no panic), a saved key
never sent to another OpenAI-compatible server or a keyless one, routes behind one host kept
apart (cache and consent), `*.localhost` not taken for this PC, keyless private network
servers, the output limit for every OpenAI-compatible base, Gemini thinking levels only for
the documented models and every `finishReason` class, enum and model names kept by the
sanitizer, failed requests not counted toward the cap and the cap in force carried by the
reason, and lines with IDs kept local (no Brackets retry at the service, more tokens than
placeholders never sent).

### Review fixes (2026-10-21)

Two reviews (security and privacy; correctness and UX) of the first version found 16 issues; all
are fixed, each with a test:

| Finding | Fix |
|---|---|
| A DeepL answer with `&amp;`, `&lt;` or `&gt;` before three CJK or Hangul characters (or four Cyrillic letters) panicked in `deepl::unescape` (a string cut at byte 12); the release build aborts on a panic, so another player's line could close Translate Companion | the `;` is searched byte-wise (ASCII, always a char boundary); a `catch_unwind` guard would not help under `panic = "abort"`, so the cause itself is fixed |
| The OpenAI-compatible key was stored per provider: a saved OpenAI key went as `Authorization: Bearer` to any base URL typed later, even plain http to this PC | keys are stored per service id (host, port and base path); another server has no saved key until one is typed for it |
| `*.localhost` names counted as this PC (exempt from the http check, the key and the proxy), but Windows resolves them through DNS | only `localhost` itself and loopback literals count |
| The Brackets retry at the service, and the 10th span and later, put URLs, e-mail addresses and IDs in clear | such lines stay local with `kept_local` |
| OpenAI-compatible requests had no output limit and the cap counted the input only | an output limit of 4000 tokens, and the answer counts toward the cap |
| Header values holding the key and the key store's JSON values were not zeroed | `HttpRequest` zeroes its headers on drop and the key store zeroes its JSON strings (documented as best effort) |
| The DPAPI entropy comment promised protection from the same user's programs | reworded |
| Consent and cache covered the host only, so routes of one gateway shared them | the base path is part of the service id and the cache identity |
| Every Gemini `finishReason` but `STOP` and `MAX_TOKENS` was a refusal, and long enum names were redacted | the content filters only are refusals; enum and model names are kept |
| `thinkingLevel: minimal` went to `gemini-3.1-flash-lite`, which the thinking table does not list | levels only for the listed models |
| An OpenAI-compatible server on the home network (http, no key) could not be set up | private network address literals need no key (a key over http stays refused) |
| Failed requests counted toward the cap, so an invalid key turned into "daily limit"; old badges showed the current cap | counted only when the service answered; the cap in force goes in the reason |
