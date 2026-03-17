# LingStation Compilation Fix Plan

## Step 1: Fix effect_hosts.iter() (line 22353)
- Add lock: `if let Ok(hosts) = state.effect_hosts.lock() { for (fx_index, fx_host) in hosts.iter().enumerate()`

## Step 2: Annotate fx_host type (line 22366)
- `let fx_host: &PluginHostHandle = fx_host;`

## Step 3: Fix lifetime in track removal (lines 14557-14568)
- Scope lock before drop.

## Step 4: Remove unused mut (line 16183)

Approve to proceed with edits.

