# docs

## Demo GIF

`demo.tape` is a [VHS](https://github.com/charmbracelet/vhs) script that renders
a short, read-only demo of vpsguard (recipes, plan, audit JSON, dry-run apply)
to `docs/demo.gif`.

Render it (needs `vhs`, `ffmpeg`, and `ttyd`):

```sh
cargo build --release --bin vpsguard
PATH="$PWD/target/release:$PATH" vhs docs/demo.tape
```

Then embed it under the README title:

```md
![vpsguard demo](docs/demo.gif)
```

Install VHS with `brew install vhs` (macOS) or from the
[releases](https://github.com/charmbracelet/vhs/releases). The script uses only
read-only commands, so it is safe to render on any machine.
