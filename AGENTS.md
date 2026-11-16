# kess: agent guide

Two-player chess in the terminal. katui draws it in pixels, and the two
players exchange moves end-to-end encrypted through public MQTT brokers, with
no server of our own.

## Layout

- `src/rules.rs`: the game record over shakmaty, with the draws shakmaty
  leaves to the caller (fivefold repetition, the 75-move rule).
- `src/bot.rs`: the built-in opponent, a 3-ply alpha-beta search over
  material and center control.
- `src/table.rs`: one round as both players see it. Each player only changes
  the facts of their own side, and `merge` checks every fact the opponent
  sends against our own copy of the rules and clocks.
- `src/wire.rs`: the room code, the topic and key we derive from it, and the
  sealed messages.
- `src/net.rs`: the MQTT link. It joins every broker in `BROKERS` at once
  and keeps our latest message retained, so a player who reconnects gets it.
- `src/art.rs`: the bundled board textures, piece sprites and the render
  camera they were made with.
- `src/camera.rs`: the orbit camera, matched to the render camera at rest.
- `src/stage.rs`: draws the board texture in perspective and composes the
  pieces, reflections and marks for one camera.
- `src/fx.rs`: effects started by moves: sparks, shockwaves, banners.
- `src/text.rs`: the bundled fonts and the HUD drawing on the finished frame.
- `src/runner.rs`: our game loop over katui's runtime: pixel density, mouse,
  and no redraws while nothing changes.
- `src/app.rs`, `src/app/hud.rs`: input, the online flow and the HUD.
- `src/sound.rs`: every sound, synthesized from the piece set's material.
- `assets/render.py`: the Blender script that renders `assets/`.

## Commands (the justfile is the single source of truth)

- `just check`: clippy with CI's denials; run after every change
- `just ci`: the gate; must pass before any commit
- `just fix`: clippy --fix + fmt, the mechanical repairs
- `just test`: unit tests; `just test-online` adds the tests that meet
  through the real brokers
- `just deny` / `just typos` / `just dupes` / `just machete`: the extra CI
  gates (licenses and advisories, spelling, copy-paste duplication, unused
  dependencies)
- `just quality`: lizard's complexity limits (cyclomatic 15, 100 lines, 8
  parameters per function) and jscpd's duplication gate; part of `just ci`
- `just art`: render the boards and pieces again with Blender, headless
- `just screenshots`: render the README screenshots headless
- `just hooks`: install the prek hooks, once per clone

## The loop

1. Keep the change small; one concern per commit.
2. `just check` after every change, `just ci` before committing.
3. Commit with `git commit`. The prek hooks run fmt, clippy and the
   commit-message gate. If a hook fails, fix the cause. Never commit with
   `--no-verify`.
4. Commit messages: one conventional line, `<type>: <lowercase imperative
   summary>` with types feat, fix, perf, refactor, test, docs, chore, ci,
   build, revert.

## Rules

- Code is done only when `just ci` passes. Run it, don't assume.
- No `unwrap`, `panic!` or `todo!` in non-test code; clippy denies them.
  `unsafe` is denied.
- Errors: one typed `thiserror` enum per module. No `Result<_, String>`.
- Never trust the opponent: a fact from the network goes through
  `Table::merge` or `Table::restore`, never straight into the state.
- Bump `wire::PROTOCOL` whenever a message changes shape.
- Changing `assets/render.py` means running `just art` and committing the
  renders with it.
- Comments explain why, never what; default to zero.
- Prose in docs, commits and comments: declarative, terse, no em dashes.
- Nothing personal in the repo: no real names, handles, emails, local
  machine paths or assistant attribution.
- Do not weaken a gate to make it pass. Fix the code, or change the rule in
  a commit of its own with the argument.
