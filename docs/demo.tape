# gli demo, rendered by VHS (https://github.com/charmbracelet/vhs).
#
#   cargo build --release --bin gli
#   PATH="$PWD/target/release:$PATH" vhs docs/demo.tape
#
# Produces docs/demo.gif. Read-only commands only (no apply), safe to run
# anywhere: plan and audit inspect the host but change nothing.

Output docs/demo.gif

Set Shell "bash"
Set FontSize 18
Set Width 1100
Set Height 680
Set Padding 18
Set Theme "Dracula"
Set TypingSpeed 45ms

Hide
Type "cd $(mktemp -d) && export C=$OLDPWD/examples/configs/gli.toml" Enter
Ctrl+L
Show

Type "# Configure and secure a Linux VPS from one file" Sleep 800ms Enter
Sleep 500ms

Type "gli recipes" Sleep 400ms Enter
Sleep 2s

Type "# Preview what would change (read-only)" Sleep 600ms Enter
Type "gli plan -c $C" Sleep 400ms Enter
Sleep 3s

Type "# Compliance score as JSON, for CI or dashboards" Sleep 600ms Enter
Type "gli audit -c $C --json | head -20" Sleep 400ms Enter
Sleep 3s

Type "# Apply is idempotent, previewed, and reversible" Sleep 800ms Enter
Type "gli apply -c $C --dry-run" Sleep 400ms Enter
Sleep 3s

Sleep 1500ms
