# VLM judge: generate until N images pass

Status: **draft, nothing built**. A proposal to fold into DESIGN.md once
settled. It builds on the chat server of Prompt rewriting (Qwen3-VL-8B-Instruct
in KoboldCpp, with vision) and on Scheduling.

## The idea

A run today generates a fixed number of seeds and leaves every image to the
user's eye. For criteria that are easy to state and tedious to check ("the
character has exactly 2 legs", "no text in the image", "one cat, not two"),
the chat model can look at each image as it comes and answer, and the run
keeps going until enough images pass:

```
          ┌───────────── more seeds (as many as the pass rate says) ◄──────────┐
          ▼                                                                    │
Run ──► sd-server ──image──► stored, in Review ──► JUDGE (Qwen3-VL) ──► passed? ┤
                                                   questions × image            │
                                                   P(yes) from logprobs         ▼
                                                                        N passed: Finish & stop
```

- **Generate until 10 pass, at most 100 seeds**: the run's goal instead of
  its seed count.
- Each image is asked the goal's **questions**; the answer is not the word
  the model writes but the probability it gives each answer (its logprobs),
  so a pass has a threshold and a score to sort by, not a coin flip.
- It is a filter, not a reviewer: every image still goes to Review
  unreviewed, with the judge's verdict on it. The user's rating stays the
  only rating (see Verdicts in Review).
- It is **rejection sampling**, not prompt tuning: the run's prompt and
  settings never change (DESIGN.md: a run's prompt is fixed and
  reproducible). Changing the prompt when too few pass is a separate,
  confirmed step (Adapting the prompt, later).

## Criteria

A goal has one or more criteria; an image passes when all of them do.

| Kind | Asks | Reads | Passes when |
|---|---|---|---|
| `YesNo` | "Does the character have exactly two legs?" | P(Yes), P(No) | P(wanted) ≥ threshold |
| `Count` | "How many legs does the character have? Answer with one digit." | P(0) … P(9) | P(count in range) ≥ threshold |
| `Choice` | "Which is the cat's colour: A) black B) white C) orange D) other?" | P(A) … P(D) | P(wanted options) ≥ threshold |

```rsn
// a goal, on the Generate form and in run.rsn
Until {
    passes: 10,
    max_seeds: 100,
    criteria: [
        Count { question: "How many legs does the character have?", want: Exactly(2), threshold: 0.8 },
        YesNo { question: "Are any of the character's legs or feet deformed, merged or extra?", want: No, threshold: 0.7 },
    ],
}
```

- `Count` reads the whole answer distribution in one ask, which is worth more
  than "exactly two? yes/no": it says the image has "4 legs (0.71)", and
  "2 or 3" is a range (`want: Range(2, 3)`, `AtLeast(1)`, `AtMost(0)`). Qwen's
  tokenizer writes every digit as its own token, so 0–9 is one token each;
  10 and more is asked as "9 or more" (`9` means 9+).
- `YesNo` is the general case. VLMs lean towards Yes, so a criterion is
  better phrased with the wanted answer **No** where it reads naturally ("any
  deformed?" → No), and a strict one can be asked both ways ("exactly two?"
  Yes and "more than two?" No) and averaged; the editor offers **Ask both
  ways**.
- `Choice` options are letters so each is one token.
- Saved criteria (**Judge presets**, `judges.rsn` in the config folder, like
  `export.rsn`) are named sets: "Two legs", "No text", "Single subject". A
  question may use the run's `{blanks}` values, so a sweep's "a {animal}" can
  ask "Is this a {animal}?".

## Asking with logprobs

One ask per criterion per image, through the PE queue (`pe::Client`), a new
job `judge` beside `rewrite` / `describe`, with its own system prompt and
sampling (DESIGN.md, PE server and requests):

```json
{
  "messages": [
    {"role": "system", "content": "You check generated images. Answer with only the answer: Yes or No, one digit, or one letter."},
    {"role": "user", "content": [
      {"type": "image_url", "image_url": {"url": "data:image/png;base64,…"}},
      {"type": "text", "text": "How many legs does the character have? Answer with one digit (9 for nine or more)."}
    ]}
  ],
  "max_tokens": 1,
  "temperature": 1.0, "top_k": 0, "top_p": 1.0, "min_p": 0,
  "logprobs": true, "top_logprobs": 10,
  "stream": false
}
```

The first token's `top_logprobs` (`choices[0].logprobs.content[0].top_logprobs`,
`[{token, logprob}]`) give the answer distribution:

- Each answer's probability is the sum over its spellings: `Yes`, `yes`,
  ` Yes`, `YES` (and `Y`) for Yes; `2`, ` 2`, `two` for 2. Tokens that are
  none of the answers are the **off-format mass**.
- **Score** = P(wanted) / P(all answers): normalised over the answers, so the
  off-format mass doesn't count against it.
- **Coverage** = P(all answers). Below 0.5 the model didn't answer in form
  (it started "The image shows…"); the criterion is **unsure**, which fails
  the image, and is listed as such so a badly worded question is found.
- One token, not streamed: an ask is reading the image and the question and
  one step. Answers are never sampled from; the sampler settings above are
  only so the probabilities reported are the model's own (see Open
  questions).
- The image goes first and the questions for one image are asked one after
  another, so KoboldCpp's reuse of a shared request start reads the image
  once per image, not once per question (as the asks of a sweep do now).
  Images are sent at 768 px, as for describing (512 px loses legs and
  fingers; see To try).
- A transparent image is flattened over a checkerboard first, as for the
  other asks.
- Qwen3-VL-8B-**Instruct** answers at once. A Thinking model would spend its
  one token on `<think>`; such a model is refused for judging ("judging
  needs a model that answers without thinking").

A server without logprobs (an `OpenAiChat` that ignores the field) falls back
to the written answer: score 1 or 0, coverage 1, and the goal says "no
probabilities from this server: the threshold doesn't apply".

## Generating until enough pass

A goal is a run's, set on the Generate form: **Seeds** becomes **Seeds** /
**Until** (a toggle); **Until** shows **[10] pass, at most [100] seeds**, the
criteria (a Judge preset ▾ or **Edit criteria…**) and **Judge existing…**
(below). Queue makes the run as now, with `until` in its `run.rsn`.

- **Seeds as needed**: the run doesn't queue `max_seeds` at once. It adds
  seeds (a revision's seeds, `Library::add_seeds`, continuing after its
  highest) so that the seeds pending, generating and waiting for judgment
  are about as many as should bring the passes still wanted:

  ```
  wanted   = passes − passed
  rate     = (passed + 1) / (judged + 2)        // a prior of 1 in 2 at the start
  ahead    = ceil(wanted / rate)                // pending + in flight + unjudged
  ahead    = clamp(ahead, 2 × servers, max_seeds − used)
  ```

  recomputed after each judgment. A run whose criteria pass 1 in 5 has about
  50 seeds out for 10 passes at the start, fewer as passes come. The
  scheduler treats it as any run (round-robin, Sweep by sweep, One at a
  time); only its count grows.
- **Done**: at `passes` passed, the run does **Finish & stop**: pending seeds
  dropped, the image generating kept and judged (it may pass too: 11 of 10
  is fine). The messages say "fox-cafe r03: 10 passed of 37 (27%)".
- **Out of seeds**: at `max_seeds` generated and judged, it stops short and
  says so ("6 passed of 100; the most common failure: Count legs 4 (58
  images)").
- **Hopeless**: after 20 judged with none passing, it pauses (pending seeds
  held, not dropped) and asks: **Keep going**, **Stop**, or (later) **Adapt
  the prompt…**. Most often the question is worded badly or the prompt can't
  make it; going on would spend an hour finding out.
- **Judging lags generation**: the judge is far quicker than generating on a
  GPU (an ask ~0.5–1 s, an image ~34 s), so it keeps up. On a CPU chat
  server an image's first ask is ~15 s (reading the image ~13 s) and each
  further question ~1 s: it still keeps up with two GPUs. In **swap** mode
  (chat and images taking turns on one card, ~5 s and ~9 s a swap) the
  images are judged in batches: after every 8 images or when the run waits
  on judgments to know how many more to queue, not image by image.
- A restart resumes it: `run.rsn` has the goal and the judgments; images
  stored and not judged are judged first.
- **Jobs** and the jobs popover show the goal instead of a seed count: "7 /
  10 passed · 26 judged (27%) · 3 to judge · ~11 more seeds"; the bar is the
  passes. The judging shows in Jobs' Prompt enhance section as one block
  per run (**Judge fox-cafe r03**, "4 to judge").

## Verdicts in Review

The judge doesn't rate. The folder is still the verdict, and only the user
moves an image there (DESIGN.md, The two review queues).

- An image's **judge chip** (Review's single view, grid tiles, contact
  sheet): ✓ or ✗ and the score of its weakest criterion; its tooltip every
  criterion's answer ("legs: 2 (0.94) · deformed: No (0.88)").
- Review's bar gets a **Judge** filter: Passed / Failed / Unsure / Not
  judged / All (default: All) and **Sort by score**, so 10 passes of 37 can
  be rated first and the rest seen later or not at all.
- **Rate the failures bad** (opt-in per goal, `failed: RateBad`): an image
  failing is moved to `reviewed/bad/` with the tag `judge` ("rated bad by the
  judge"), so the queue holds only passes. Off by default; `4` (undo) and the
  Output library's Rejected view bring one back.
- Every verdict is kept: the user's rating against the judge's answer is the
  data for calibrating (below).

## Judge existing images

**Judge with…** on images' right-click menus (Review's selection, the
Output library, a folder view) and on a run in Jobs asks a Judge preset of
images already made: chips and the filter as above, no seeds queued. It's how
a criterion is tried on a run before trusting it with a goal, and how a run
of 200 made yesterday is sorted by "two legs".

## Calibration

A threshold of 0.8 means little until the user's own ratings say how often a
0.8 is right. VLMs are poor at counting limbs and fingers and overconfident
about it; how poor at this resolution, on Qwen-Image's kinds of mistakes, is
to be found out (To try).

- **Judge presets ▸ Check against my ratings**: for a preset, the images it
  judged that the user has rated (good / neutral count as "pass" for this,
  bad as "fail"): agreement, how many passes were rated bad, how many
  failures were rated good, and the score at which the two lists separate
  best, offered as the threshold ("at 0.86: 9 in 10 of your good ones pass,
  1 in 10 of your bad ones").
- Bad ratings aren't always about the criterion (a two-legged fox rated bad
  for its face), so this is a hint for setting the threshold, not a test.

## Adapting the prompt (later)

When few pass, the failures say why ("4 legs" in 58 of 63). **Adapt the
prompt…** (the Hopeless question, and a goal's menu in Jobs) asks the
`rewrite` job with the prompt, the criteria and a summary of the failures
("most images show four legs; wanted: two") for a prompt more likely to
pass, and opens the rewrite window as Rewrite does, with its diff. **Use for
the rest** queues a **new run** in the project with that prompt, carrying
the goal's remaining passes; the old run stops (Finish & stop). The passes
of both count to the goal, shown together in Jobs.

- Always confirmed: the app never changes a prompt on its own.
- A new run, not a revision: a revision holds only seeds; prompt and
  settings belong to the run (`model.rs`, `Run` / `Revision`), and the recipe
  records each image's own.
- An automatic mode (adapt after every 20 failures, up to 3 times) is
  possible on top, but not proposed: it is where the loop stops being
  reproducible and starts costing hours unwatched.

## Data

- `run.rsn` gains `until: Until { passes, max_seeds, criteria, failed }` and
  `judgments: [Judgment { rev, seed, passed, answers: [Answer { criterion,
  score, coverage, top }], judge: JudgeTag { server, model, system_prompt_hash
  }, done }]`, where `top` is the most likely answer ("4"). They're about the
  run, like `timings`; a later re-judging (another preset) adds another
  `Judgment` for the seed, the goal's being the first with the goal's
  criteria.
- Not in the recipe: the recipe says how an image was made, and is in the
  file forever; a judgment is an opinion about it that may be redone.
- `Judge with…` on images outside a goal stores its judgments in the image's
  run as well (every output has one), with no goal.
- `judges.rsn` (config folder): the Judge presets. `config.rsn`'s `pe` gets
  the `judge` system prompt and sampling as the other jobs.

## Open questions

- **KoboldCpp's logprobs**: it has returned OpenAI-style `logprobs` from
  `/v1/chat/completions` for a while, but how many alternatives
  (`top_logprobs` may be capped at 5, which is tight for a 0–9 count),
  whether they are the model's probabilities or after its samplers
  (temperature, top-k, min-p), and whether a multimodal ask returns them, is
  to be read in its 1.123.1 source and tried. If capped at 5, `Count` reads
  the 5 likeliest counts and treats the rest as 0 (coverage says how much is
  missing).
- **Grammar instead of normalising**: KoboldCpp takes a GBNF `grammar`, which
  would force the answer into `Yes|No` or `[0-9]`. Do the logprobs it
  reports then come from before or after the grammar's mask? Before, they
  are what's wanted and coverage is still meaningful; after, they're the
  same as normalising. Normalising works on any OpenAI server, so it's the
  default either way.
- **One ask with every question** (a JSON answer) would read the image once
  even where the request start isn't reused, but the answers' logprobs are
  then mid-answer tokens, after the model's own earlier answers. One token
  per question is kept until measured otherwise.
- **The goal across a sweep**: "10 pass in every style" (a goal per run of
  the sweep) or "10 pass in all" (one shared goal). Per run first.
- Is "pass" one goal's AND of criteria enough, or are weights / any-of
  wanted? AND until someone needs more.

## To try

- On 100 hand-labelled images of characters (legs and hands): Qwen3-VL-8B's
  `Count` against the true count, at 512, 768 and 1024 px; where it goes
  wrong (hidden legs, a fifth leg of the same colour, tails). Then the same
  with the Q8_0 model, and with a crop to the subject's box (Qwen3-VL can
  give boxes) before counting.
- Yes-bias: "exactly two?" vs "more than two?" on the same images; whether
  asking both ways and averaging separates passes from failures better.
- Time per judged image with three criteria on one R9700 beside sd-server,
  on CPU, and in swap mode on one card.
- Whether the off-format mass is ever large with the system prompt above, or
  the coverage check never fires.
