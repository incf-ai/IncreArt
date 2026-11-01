# Contributing

Thanks for wanting to help. IncreArt takes **issues, not pull requests**.

## What helps most

- **Bug reports.** Something broke, looked wrong, or was slower than it
  should be. Use the *Bug report* form.
- **Ideas and requests.** A workflow that's awkward, a backend or GPU that
  should be supported, a missing feature. Use the *Feature request* form.
- **Setup trouble.** If the [setup guides](docs/setup/README.md) didn't get
  you running, that's a bug in the guides. Report it as a bug.

For questions, or quick help while you set up, ask in
[#increart on the KoboldAI Discord](https://discord.gg/7D4jgg6J6Z).

## Why no pull requests

IncreArt has one maintainer, and the design is kept deliberately coherent
(see [docs/DESIGN.md](docs/DESIGN.md)). Reviewing and merging outside code
costs more than writing the change from a clear issue, so pull requests are
turned off for this repository. A good issue gets your change made just as
well.

If you've already written a fix, describe it in the issue: what was wrong,
what you changed and why. A short snippet or a link to your fork's commit
is welcome as an illustration. It won't be merged as is, but it helps.

## A good bug report

- What you did, what you expected, and what happened instead.
- Your OS, GPU(s) and VRAM, and the image backend (sd-server or KoboldCpp,
  with version).
- The server log: the app's **Logs** view shows it, or the log file it
  names. Trim it to the part around the problem.
- A screenshot if it's about something you see.

Leave out anything private: prompts or images you don't want public,
absolute paths that name you, tokens.

## Licence

IncreArt is MIT licensed (see [LICENSE](LICENSE)). Anything you post in an
issue may be used to improve it under that licence.
