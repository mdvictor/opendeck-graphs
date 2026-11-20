# Interstitial Hardware Graphs for OpenDeck

Interstitial Hardware Graphs displays live hardware readings on OpenDeck keys and encoder displays. This fork extends [Hardware Graphs v2.2.0 by Victor Marin](https://github.com/mdvictor/opendeck-graphs/tree/v2.2.0) with a Windows x64 sampler, glass and solid instrument faces, and support for the joined VSDinside N4 Pro strip. The graph action also works independently.

## Install and use on Windows

Build the Windows ZIP below, then use OpenDeck's **Plugins → Install from file**. Drag **Interstitial Hardware Graphs → Hardware Graph** onto a key or encoder. Choose a metric and, for a GPU reading, an adapter. Automatic selection favors the adapter with the most dedicated memory. A missing selected adapter displays **No data**. History holds up to 60 samples and targets one update per second.

Use **Second reading** to display two histories on one encoder. On the N4 Pro, pressing that knob cycles through the overview and individual traces. A single-metric knob toggles between its gauge and trace. With the companion Interstitial N4 plugin and a joined strip, **Appearance → Strip alignment** adjusts all four zones in one-pixel steps. Ambient rings can follow the matching temperature, RAM or network reading.

## Sensors and diagnostics

Windows PDH supplies CPU and network rates. WMI covers warm-up and unavailable counter groups. NVIDIA readings prefer NVML, with Windows GPU counters or LibreHardwareMonitor providing supported fallbacks. Disk, RAM and other hardware readings depend on their respective providers. Run `oa-graphs.exe --probe` to inspect adapters and readings. Unavailable measurements appear as JSON null.

CPU package temperature can come from LibreHardwareMonitor WMI or the optional tray provider in `build/temperature`. To build the provider, pass an extracted compatible LibreHardwareMonitor release to `build/temperature/build.ps1`, make its runtime assemblies available beside the output, and use the driver and permissions required by that release. The helper can start an already-installed PawnIO service. If configuring elevated startup, keep the helper and its dependencies in an administrator-controlled directory. OpenDeck itself runs as the desktop user.

The optional WebSocket source accepts numeric telemetry and initialization messages. OpenDeck profile settings can contain its URL and API key. Keep profiles private when sharing logs or filing issues.

## Optional artwork and launcher catalog

### Existing installations

Interstitial has new plugin and action IDs. Keep a backup of your existing profiles. To try it alongside an older graph action, duplicate a profile and assign Interstitial actions to the copy. To reuse your own optional settings, copy only the artwork and launcher catalog you choose into the Interstitial configuration directory. The public package starts with an empty launcher catalog.

Bring your own upright 800 × 480 PNG as `%APPDATA%\Interstitial\live-wallpaper.png`. The Windows N4 pairing also understands `day-wallpaper.png` and `night-original.png`. Restart both plugins after replacing images. **Appearance** controls finish, tint and transparency.

The Windows **App Launcher** reads `%APPDATA%\Interstitial\launchers\catalog.json`. Use `Get-StartApps` in PowerShell to find an installed app's ID. Each catalog entry needs a matching PNG in that directory. For example, replace the ID below and supply an image named `icon-0.png`:

```json
[{"id":"YOUR_START_APP_ID","label":"My app","slot":0}]
```

Restart the graph plugin, assign **App Launcher** to a key and select the app in the inspector. **Show app name** trades icon size for a caption.

## Build and test

On Windows x64, install stable Rust with the `x86_64-pc-windows-msvc` target, Visual Studio C++ Build Tools and PowerShell 7. Run:

```powershell
cargo test --release --locked --all-targets --target x86_64-pc-windows-msvc
cargo clippy --release --locked --all-targets --target x86_64-pc-windows-msvc -- -D warnings
./build/package-windows.ps1
```

The packager finds the x64 VC release runtime through Visual Studio Installer. Pass `-CrtDirectory` explicitly for a different installation. The ZIP in `artifacts/` includes the graph action and the Windows launcher. Linux uses the upstream sensor path. The bundled DejaVu font provides a fallback. On Windows the renderer can also use compatible fonts already installed by the user. See the [project license](LICENSE) and [DejaVu notices](fonts/LICENSE-DejaVu.txt).
