# VeraX extension

VeraX is VERA20K's own first-party extension namespace, separate from the Ares, Phobos and
NPatch compatibility capabilities. Build it explicitly:

```sh
cargo run --release --bin vera20k --features verax
```

## AutoLoad

Ctrl+D issues standard Enter orders for the current selection: selected infantry and vehicles are
balanced by type into the selected carriers that are within 30 cells, have `Passengers=` room and
accept the passenger's `Size=` under `SizeLimit=`. Each type takes turns, the closest hulls fill
first, and a loaded carrier or a vehicle already assigned one role cannot also take the other.
Distance and engine admission are checked for each pair before room is reserved. Orders are ordinary Enter
missions, so a replay, a save or another client sees the same commands; the engine's own
`Receive_Radio` 0x0F admission decides every boarding. Buildings that garage through their own
Garrison/Occupy channels are not part of this chord.

The command is registered as `AutoLoad` in the keyboard page's `vera20k` category, so it can be
rebound or cleared like a retail hotkey. Ctrl+D is installed as the reload default when neither
AutoLoad nor Ctrl+D already has a saved binding, and an explicit `AutoLoad=0` keeps it cleared.
The assignment is written to the profile with the other bindings.

## Map hot reload

Without this extension the Skirmish map list is scanned once at startup, so a map copied into the
game directory is only visible after restarting. A VeraX build rescans the directory each time the
Skirmish Choose Map dialog opens (`SessionClass::ScanMultiplayerMapFiles` @ `0x00699980` again), so
the new loose `.pkt`, `.yrm` or `.yro` map and its rows appear without a restart. Rows already
listed are not duplicated, and the current selection keeps following the same map file when the
rescan moves its index.

This is not a directory watcher: the list changes on the next open of the chooser. Network lobbies
keep their launch snapshot, because the map list is local and every peer must still compare the same
catalog.

## Namespace

`--features verax` compiles `rules::VERAX_ENABLED`. VeraX-only readers take their keys and
commands under `#[cfg(feature = "verax")]`. AutoLoad's distribution and range are VeraX input
policy; its ordinary Enter orders and boarding admission reuse the native-derived engine owners
(`FootClass::ClickedAction` at `0x004D74E0`, `UnitClass::Receive_Radio` at `0x00737430`).
VeraX is one of four orthogonal capabilities: it combines with `ares`, `phobos` and
`npatch` (`--features ares,phobos,npatch,verax`). The compatibility rule and the pattern for two
extensions touching one native site are in
[NPatch compatibility](npatch.md#cross-extension-compatibility).
