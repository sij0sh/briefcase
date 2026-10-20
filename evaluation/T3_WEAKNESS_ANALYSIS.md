# T3 weak points and recommended fixes

Date: 2026-10-19. Run: `t3-eval-work/fast-1019` (uncommitted tree on `a191855`,
binary sha256 `86f6b70c…`, `--workers 6 --serve`). Its scores are identical to
`fresh-1019`. The simulation scripts and full candidate dumps are in
`t3-eval-work/fast-1019/an/`.

## 1. Conclusions

1. **Ranking is the bottleneck, not discovery.** On the joined DB, 55 of 56
   required paths are candidates, but only 36 rank in the top 32 and 13 rank
   below 64. Only 1 miss on joined, and 2 on text-only, is a file that never
   became a candidate.
2. **In the brief, the import layer adds noise.** If the TypeScript profile's
   import-derived signals are removed from the joined scores, the result is
   exactly the text-only result (40/56, the same in every scenario). The import
   layer costs −2 recall and about 5× brief latency (9.8 s vs 1.8 s warm median).
3. **One rule with no parameter fixes most of that:** an import edge may
   corroborate task relevance but may not exceed it. Capping each import signal
   at the candidate's own lexical or anchor strength gives joined **38 → 41/56,
   6 → 8/14 passes, and 25 → 29/44 supportable claims**. Text-only is unchanged,
   because it has no such signals.
4. **The 8 KB manifest budget is still the cheapest lever.** About 2.9 KB of
   each manifest is fixed metadata. A 48-file cap would add +4 joined and +2
   text-only. This is NEXT_IMPROVEMENTS #1/#2, still not done, and this run
   confirms it.
5. **About 9 required files are missed by every arm.** They are vocabulary
   gaps: the task describes a symptom and the code uses a domain name. No
   deterministic re-weighting I tried reaches them. The engine's existing
   answer is the continuation protocol (`neighbors`/`search`/`page`). That
   protocol has not been re-measured since calls became about 15× faster.

## 2. What the eval asks, and how the engine answers

There are 14 scenarios with 56 authored required paths and 44 outcome claims.
Each claim is a fact, condition, exception or verification. Each has witness
passages, kept in `t3-outcome-claims.json` and used only by the scorer. A
scenario **passes** when every required path is selected. A claim counts as
**supportable** when every witness file is selected. Having the file is
necessary but not sufficient to reach the right conclusion.

| Scenario | Duty | Target given | Required | Joined | Text | Lexical | Claims supportable (J / T) |
| --- | --- | :-: | --: | --: | --: | --: | --: |
| S1 terminal bounds | test | yes | 2 | 2 | 2 | 2 | 3/3 · 3/3 |
| S2 keybinding owners | explain | yes | 3 | **3** | 2 | 1 | 3/3 · 2/3 |
| H1 RPC permission | review | yes | 5 | 4 | 4 | 2 | 2/4 · 2/4 |
| H2 checkpoint finalization | debug | yes | 4 | 1 | 2 | 1 | 1/3 · 2/3 |
| H3 huge history item | test | yes | 3 | 2 | **3** | 1 | **0/3** · 3/3 |
| H4 stale PR display | debug | no | 4 | 1 | 0 | 0 | 0/4 · 0/4 |
| H5 redacted settings save | extend | yes | 4 | 4 | 4 | 4 | 3/3 · 3/3 |
| H6 rollback capability | review | yes | 7 | 3 | 3 | 4 | 1/4 · 1/4 |
| H7 offline webhook retry | review | no | 6 | 3 | 3 | 3 | 1/3 · 1/3 |
| H8 flapping backoff | debug | no | 3 | 3 | 3 | 3 | 3/3 · 3/3 |
| H9 Linux portal ordering | refactor | yes | 4 | 3 | **4** | 4 | 3/3 · 3/3 |
| H10 attachment retry | debug | yes | 3 | 1 | 2 | 2 | 0/3 · 0/3 |
| H11 monitor sequence | debug | no | 4 | 4 | 4 | 3 | 2/2 · 2/2 |
| H12 mobile reveal patch | debug | yes | 4 | 4 | 4 | 3 | 3/3 · 3/3 |
| **Total** | | | **56** | **38** | **40** | **33** | **25/44 · 28/44** |

Observations:

- The engine handles a scenario well when the task names the subsystem or the
  target is given and its companion test is the evidence (S1, H5, H8, H11,
  H12). It handles it badly when the task is phrased as a user-visible symptom:
  H4 ("cached multi-request stack" is a PR stack), H6 ("rewind" is
  `CommandPolicy`), and H7 ("duplicate runs avoided" is `RelayDeliveryProof`).
- Path recall and claim support can disagree sharply. On joined, H3 recalls
  2/3 paths but supports 0/3 claims, because `threadHistoryPaging.ts` witnesses
  all three. H9 recalls 3/4 but supports 3/3, because the missing file is not a
  witness. Pass/fail on paths alone misjudges both cases.
- `outcome` is `partial` or `unknown` in 14/14 scenarios, including the ones
  that pass. That is correct under the engine's contract ("complete for the
  retained evidence scope"), but it tells an agent nothing about when to keep
  investigating.

### Miss taxonomy

| State | Joined (18 misses) | Text-only (16 misses) |
| --- | --: | --: |
| Not a candidate | 1 (`RunFinalizationService.test.ts`) | 2 (+ `clientRpcPermissions.ts`) |
| Below minimum score (24) | 4 | 5 |
| Candidate, rank ≤ 64, cut | 6 (ranks 11, 32, 39, 49, 51, 57) | 2 (ranks 29, 54) |
| Candidate, rank 65–150, cut | 5 | 5 |
| Candidate, rank > 150, cut | 2 | 2 |

Files missed by **all three** arms: `RunFinalizationService.ts` and its test
(H2), `thread-pr-presentation.ts`, `use-thread-pr.test.ts` and
`threadPullRequestCompatibility.ts` (H4), `CommandPolicy.ts`, its test and
`AntigravityAdapterV2.ts` (H6), and `RelayDeliveryProof.ts` (H7). In 7 of 18
joined misses, no task word (even stemmed) appears in the filename.

## 3. Weak points, with evidence

### W1. Import edges score additively and reward hubs (joined only)

- Score is the sum of the strongest signal per facet plus 8 per extra facet
  (`retrieval.rs` `contribute`). One import edge therefore adds a whole
  `dependency` facet, often 35–80 points, to any file it reaches, whatever the
  file's relevance to the task.
- Hub damping is logarithmic: `bits(degree) × 9`. A degree-27 hub only cuts
  `TESTS` from 100 to 55.
- Worked case (H9, a desktop task): `serverRuntimeStartup.autoPull.test.ts`
  enters only through a lexical match on "startup" and becomes an expansion
  starter. It then passes a 55-point `TESTS` signal (and a 35-point
  `IMPORTS`) to all 27 of its targets. Seven server files outrank the gold
  `DesktopLinuxUrlHandler.ts`, among them `Orchestrator.ts`, `EffectWorker.ts`,
  `keybindings.ts` and `ServerEnvironment.ts`. Across 14 scenarios, 145 of 448
  joined slots go to files that text-only does not select. Barrels such as
  `packages/contracts/src/index.ts` recur.
- The edge kinds involved: 2,507 `IMPORTS` (class `evidence`) and 2,500
  mirrored `DEPENDS_ON` signals, plus `test_IMPORTS`/`test_DEPENDS_ON` `TESTS`
  signals.

### W2. The fixed manifest budget limits delivery to 32 files

- Manifests use 7.3–7.9 KB of 8 KB, of which 2.7–2.9 KB is non-file metadata.
  Every scenario delivers exactly 32 files.
- The gold sits just past the cut. Simulated, raising the cap from 32 to 48
  gives joined 38 → 42 and text-only 40 → 42. At cap 64: 43 / 43.

### W3. Implementations and their companion tests are delivered separately

- In joined H3, H4 and H9, and text-only H10, a required file's companion test
  or implementation *is* selected but the required file is not (for example,
  `threadHistoryPaging.test.ts` in but `threadHistoryPaging.ts` out).
- Pairing companions inside the 32-file cap would add about 12 files per
  scenario. It loses −2 on both arms, because it displaces other gold.

### W4. Role misclassification: `hooks/` and `bin/` make code into `script`

- `source::artifact_role` classifies any file under a `hooks` directory as
  `script`. In T3 that covers 78 TypeScript files, including the H7 gold
  `infra/relay/src/hooks/HookForwarder.ts` and `HookInboxObject.ts`.
  `src/hooks/useX.ts` in any React codebase is affected the same way.
- Script files compete in the `support` band with docs and config instead of
  the `impl` band. In a debug or review duty they also lose the +6 required-role
  bonus.

### W5. The lexical channel can't see inside identifiers

- FTS5 `unicode61` tokenizes `RunFinalizationService` as a single token. The
  query `"finalization"` matches 15 files, and the file named for it is not one
  of them.
- The substring `matches()` re-score sees subwords, but only for files the FTS
  already returned. Term counts are unweighted, so common words count the same
  as rare ones.
- A simulated name-part signal did *not* help (§4), so this is a real gap
  without a demonstrated fix.

### W6. Latency of the joined profile

- Brief: 9.8 s warm and 14.1 s cold on joined, against 1.8 s and 2.6 s on
  text-only.
- Serve mode hides the repeated cost but not the first brief. Engine item 3,
  the storage format (about 2.7 s spent parsing view bundles), is still open.
- These latency figures came from a serial pass that ran while the other arm's
  workers were active, so they are probably inflated by a few percent.

## 4. Solutions tested by simulation

**Method.** For every scenario on both DBs, I dumped the full retained candidate
list from a real `brief` (1,004–1,144 candidates each, with every signal's
strength, depth, hub degree and basis). I then re-implemented the final scoring
and `delivery_order` in Python (`an/sim.py`). The simulator reproduces all 28
real selections exactly before any change.

Changes applied at final scoring (R1, R4, and the cap) are modeled faithfully.
Changes to expansion strength are first-order only: the frontier is held fixed,
and signals the engine discarded below strength 12 can't be revived.

| Change | Joined | Text-only | Verdict |
| --- | --- | --- | --- |
| Baseline (recomputed) | 38/56 · 6 pass | 40/56 · 7 pass | matches the run |
| **Cap import-derived signals at lexical/anchor strength** | **41 · 8** | 40 · 7 | **adopt (R1)** |
| Remove import-derived signals entirely | 40 · 7 | 40 · 7 | worse than the cap |
| Gate imports on lexical ≥ 30 | 41 · 8 | — | works, but adds a parameter |
| Cap *all* graph signals at lexical | 38 · 7 | 38 · 7 | reject: text links are precise |
| **File cap 48** | **42 · 8** | **42 · 7** | **adopt after compaction (R2)** |
| Cap imports + file cap 48 | 44 · 9 | 42 · 7 | |
| Cap imports + file cap 64 | 46 · 9 | 43 · 7 | |
| **Companion pointers outside the cap** | **41 · 8** | **41 · 8** | **adopt (R3)** |
| Cap imports + companion pointers | 42 · 9 | — | |
| Companion pairing inside the cap | −2 | −2 | reject |
| **`hooks/` code files → `impl`** | 38 · 6 | **41 · 7** | **adopt (R4), correctness fix** |
| Inverse-degree hub damping (k = 8, 6, 4) | 39 · 7 | 40 / 40 / 39 | reject: small gain, adds a parameter |
| Steeper log hub penalty (×12/15/20) | 36 / 35 / 33 | 39 / 37 / 37 | reject |
| Drop hub degree > 16 or > 24 | 39 · 7 | 38 / 39 | reject |
| Drop all depth-2 signals | 38 · 5 | 40 · 7 | reject |
| Test-duty cycle `t,i,t,i,s` | 39 · 7 | 40 · 7 | reject: 2 scenarios, overfit risk |
| Filename-part lexical signal (16 / 30) | 42 / 36 | 40 / 37 | reject |

On 56 labels, ±1–2 is noise. The only changes I treat as real are those that
are mechanistically explained, have no parameter, and don't regress either arm:
R1, R3 and R4. R2 is the documented budget effect, now confirmed a second time.

## 5. Recommendations

These are ordered by value per unit of effort. Each keeps Briefcase's identity:
deterministic, provenance-explicit signals over retained evidence, no learned
or corpus-tuned weights, and no runtime claims. None adds a new facet, index or
dependency.

### R1. Corroboration cap for import edges (small, ~15 lines in `rank()`)

- **Rule:** for `IMPORTS`/`DEPENDS_ON` signals with basis `retained_assertion`,
  and `TESTS` signals with basis `test_IMPORTS`/`test_DEPENDS_ON`, use
  `strength = min(strength, strongest lexical-or-anchor signal of that
  candidate)`. Apply it in `rank()` before sorting, so expansion and provenance
  stay untouched and the signal records both the raw and the capped strength.
  Anchors (10000) are effectively uncapped.
- **Why it fits the identity:** structure explains *how* a file relates. It
  shouldn't, on its own, assert *that* the file is relevant to the task.
- **Expected effect (simulated):** joined 38 → 41, 6 → 8 passes, 25 → 29
  supportable claims. Text-only unchanged.
- **Gate:** run the Briefcase (36) and held-out (19) sets on a frozen corpus
  snapshot. Reject R1 if either loses a path. Their Rust import edges may carry
  more true signal than T3's TypeScript barrels do.
- **If it is rejected:** stop feeding the workspace profile into brief scoring.
  Keep it for `neighbors`, `reachability` and `explain`. That also removes the
  5× brief latency (W6) without needing Engine item 3.

### R2. Compact the manifest, then raise the agent cap to 48 (small–medium)

- Move `instruction`, the full `coverage` object, and `pinned_views` behind
  one continuation, and fold `ref` into the fetch template. That frees about
  2 KB. Then default `agent_context` to 48 files under the same 8 KB budget.
- **Expected effect:** +4 joined and +2 text-only. With R1, joined reaches 44.
- **DRY:** one byte budgeter decides delivery. Don't add a second cap.

### R3. Companion pointers outside the cap (small, after R2)

- For each delivered file, if `companion_test_paths` (or the reverse,
  test → implementation) names a retained file that wasn't delivered, attach its
  path to that entry as `companion`. The pointer doesn't take a slot. The
  existing helper and its "not proof of behavior" basis are reused.
- **Expected effect:** +3 joined, +1 text-only (H3, H4, H9; H10).
- **Honesty:** the eval must count these as `pointed`, separately from
  `selected`. A pointer is a path the agent can fetch, not ranked evidence.
- Costs roughly 25–40 B per pointer, about 300–500 B per manifest, so it
  depends on R2.

### R4. Role fix for code under `hooks/` and `bin/` (trivial)

- In `artifact_role`, classify `hooks`/`bin` as `script` only when the file is
  not a code extension (git hooks have no extension). Keep the existing Rust
  `src/bin` rule.
- **Expected effect:** +1 text-only (H7 `HookForwarder.ts`). It is a
  correctness fix regardless of the score.
- Re-run the Briefcase set, because `index.rs` treats `script`/`install`
  roles specially.

### R5. Re-measure investigations under `--serve` before building anything for the hard core (no code)

- In Stage 3, all 14 investigations hit the 180 s limit at 0.9–1.4 s per call.
  Explain and page calls through serve are now about 50 ms. The continuation
  protocol is the engine's designed path to vocabulary-gap files. Its value at
  the new speed is unknown, and that should be measured before W5 work is
  justified.
- Run: `task-evaluate.py … --workers 6 --serve` with investigations enabled,
  reporting how many of the 9 hard-core files are recovered and how many calls
  it takes.

### R6. Add claim-witness coverage to the eval summary (small, in `scripts/`)

- `t3-outcome-claims.json` already holds the witnesses. Report the share of
  claims whose witness files are delivered next to path recall. That makes
  H3-type failures visible (2/3 paths, 0/3 claims). The scorer stays separate
  from prompts, as it already is.

### Not recommended now (YAGNI)

- **Embeddings or a semantic facet for the hard core.** They would bring a
  model dependency and non-deterministic ranking into an engine whose value is
  explicit provenance. Revisit only if R5 shows the continuation protocol
  can't reach these files.
- **Identifier-split FTS (W5).** It would need an index rebuild, and the
  simulated filename-part signal was neutral or harmful. Revisit after R5.
- **Hub-penalty retuning, duty-cycle changes, depth limits.** All simulated
  neutral or negative, or they depend on a parameter (§4).
- **Engine item 3 (storage format).** Only needed if the workspace profile
  stays in brief scoring after R1's gate.

## 6. Evaluation-suite gaps

- **Small n.** 14 scenarios and 56 labels from one author. Every change must
  be checked on the T3, Briefcase and held-out sets together, and the Briefcase
  corpus must be a frozen snapshot (Stage 3 lost 3/36 to drift).
- **The agent tier has no usable T3 answers.** 102 of 180 T3 investigation
  trials in `p4-agent/costs.jsonl` errored on turn 1. The new fail-fast option
  (`--agent-max-errors 1`) will stop that recurring silently, but the tier
  still has to be re-run before any claim about answer quality.
- **Investigations** were not part of this run (see R5).
- **Latency of record** should come from arms run one after another. A
  concurrent arm's workers inflate the serial pass.

## 7. Reproduce

```sh
cd t3-eval-work/fast-1019
python3 an/cands.py an/sem.db <scenario-id> an/sem-<scenario-id>.json   # full candidate dump
python3 an/sim.py          # verifies baseline reproduction and prints the baseline
python3 an/variants.py     # hub, role and cycle variants
```

The R1, R3, cap and combination simulations were inline scripts built on
`an/sim.py`. Each is a one-function `rescore` wrapper over the same candidate
dumps.

## 8. Implementation results (2026-10-19, uncommitted)

The tree was frozen at `t3-eval-work/rec-1019/tree`. New binary `4579ecd9…`, old
binary `86f6b70c…` (fast-1019). The Briefcase and held-out sets ran both
binaries on identical copies of one captured DB.

**What changed:**
- **R1:** `retrieval.rs` `corroborate()` runs in `rank()` and caps
  `IMPORTS`/`DEPENDS_ON` (`retained_assertion`) and `test_IMPORTS`/`test_DEPENDS_ON`
  signals at the candidate's lexical or anchor strength. Provenance keeps
  `uncapped_strength`. `facet_score()` is now shared with `contribute()`.
- **R2:** `max_files` defaults to 48 (request and CLI). The manifest
  instruction is shorter, and the schema is unchanged.
- **R4:** `source.rs` treats code files under `bin/` and `hooks/` as `impl`.
  Extensionless and shell files are still `script`.
- **Not done:**
  - R3 (companion pointers): deferred, see below.
  - R6: claim supportability already exists in `task-outcomes.py`
    (passage-level), so nothing was added.

| Set | Old case | New case | New 8 KB manifest |
| --- | --- | --- | --- |
| T3 joined | 38/56 · 6/14 | **44/56 · 9/14** | 42/56 · 8/14 |
| T3 text-only | 40/56 · 7/14 | **42/56 · 7/14** | 41/56 · 7/14 |
| T3 lexical-only control | 33/56 | 35/56 | — |
| Briefcase | 28/36 · 2/8 | **32/36 · 5/8** | 31/36 · 4/8 |
| Held-out | 17/19 · 3/5 | **18/19 · 4/5** | 18/19 · 4/5 |

- No scenario on any set lost a required file, and there were 0 errors and
  0 false-complete outcomes. The T3 results match the §4 simulation exactly
  (R1 + cap 48: 44/9 and 42/7).
- **Claim supportability** (`task-outcomes.py`, deterministic tier, sum of the
  per-scenario fractions over 14 scenarios):
  - hybrid: 8.08 → 9.42 on joined, 9.08 on text-only. H3 went 0 → 1, H12
    0.67 → 1, and text-only S2 went 1 → 0.67.
  - lexical: 6.67 → 7.67.
  - rg is unchanged at 2.00.
  - The p4 baseline came from an earlier `target/release` binary, so treat
    that comparison as indicative.
- **Manifest:** 33–36 of the 48 selected files fit in 8 KB, and the
  continuation reaches the rest. Fitting all 48 needs per-file compaction:
  about 147 B per entry, with `role` derivable from `ref`. That is a schema
  change and was left for a decision.
- **Latency:** joined median brief 9.9 s → 11.0 s. Four arms ran concurrently,
  so this is not clean.

**R5, measured: investigations recover nothing beyond the manifest.**
Manifest-first investigations now finish in 27–62 s instead of timing out.
But 26 of 28 stop at the 2 MB byte limit (2 at the call limit) after about
290 calls and about 33 inspected files, and they recover **0** required files
beyond the manifest on either arm. The protocol spends its budget reading
delivered files in full before it pages `/candidates`. This is now the largest
gap inside the engine's identity: the designed path to the tail (§1.5) doesn't
reach it. The next step is to fix the investigation order (page candidate
inventories and excerpts before full source reads). Don't add a new retrieval
signal first.

**R3, deferred.** With 48 files, simulated pointers add only +1 joined and +2
text-only. About 19 pointers per manifest (≈950 B) would displace about 6
ranked files from a manifest that is already byte-bound.
