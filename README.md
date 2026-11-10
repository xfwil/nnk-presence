# nnk-presence

Discord Rich Presence for **Ni no Kuni: Cross Worlds** (PC).

A small Windows tray app that passively reads your own game traffic and shows what you are doing on Discord:

```
Playing Ni No Kuni - Rich Presence
[class portrait]  Player - 10.2M CP
                  Farming at Sagartha · Ch. 3
                  00:42 elapsed
hover portrait: Engineer · Lv 218 · CP 10,216,846   hover class icon: Deck 6
```

It never touches the game process, never injects, and never sends anything to the game servers.
It only reads packets that already arrive on your network adapter, the same way Wireshark does.

## Installation

You need Windows 10 or 11, plus these three things:

### 1. Npcap (packet capture driver)

1. Go to <https://npcap.com/#download> and download the latest **Npcap installer** (`npcap-x.xx.exe`).
2. Run it. The default options are fine. Leave
   **"Restrict Npcap driver's access to Administrators only"** unticked, otherwise the app has to be run as Administrator.
3. Finish. No restart is needed.

Already have [Wireshark](https://www.wireshark.org/download.html)? It installs Npcap for you, so you can skip this step.

### 2. Discord desktop app

Install it from <https://discord.com/download> and keep it running while you play.
The browser version of Discord cannot show Rich Presence.
In Discord, **User Settings → Activity Privacy → "Share your detected activities with others"** must be on.

### 3. nnk-presence

1. Open the [latest release](../../releases/latest) and download `nnk-presence.exe`.
2. Put it in any folder, for example `C:\Tools\nnk-presence\`. The log file is written next to it.
3. Run it. The exe is not code-signed, so Windows SmartScreen may show "Windows protected your PC".
   Click **More info → Run anyway**.

No installer, no extra runtime. Delete the exe to remove it. If you turned on "Start with Windows",
untick it first, or remove `NNK Presence` from Task Manager → Startup apps.

## Usage

1. Run `nnk-presence.exe`.
2. Start the game and enter the world. Your character is picked up every time you enter the world,
   so if the app was started mid-session, go to character select and re-enter once.
3. The window shows what has been detected.
   - **Show on Discord**: untick to hide the presence.
   - **Start with Windows**: starts hidden in the tray when you sign in.
   - Closing the window hides it to the tray. Use the tray menu to show it again or to quit.

Every running Discord client (Stable, PTB, Canary) gets the presence.
Errors are logged to `nnk-presence.log` next to the executable.

### What is shown

| Field   | Source                                                                    |
|---------|---------------------------------------------------------------------------|
| Name    | character info on login                                                   |
| Class   | character info, mapped with the game's `Class_Name` table                 |
| Level   | character summary on login                                                |
| CP      | on login, updated live when it changes                                    |
| Activity | updated on every map change, from the game's `World` table (English names): open-world fields show "Farming at *map*", towns "In *map*", dungeons, raids, arenas and events their own name |
| Channel | open-world maps only: on map entry, and when you switch channel          |
| Deck    | when you switch deck (unknown until the first switch)                     |

## Troubleshooting

| Window shows | Fix |
|---|---|
| `Npcap not available` | Npcap is not installed. See step 1 above. |
| `no capturable network adapter` | Npcap was installed with admin-only access: run the app as Administrator, or reinstall Npcap with that option unticked. |
| `Waiting for the game…` while in game | Go to character select and re-enter the world once. On a VPN, also check that game traffic still goes through a normal network adapter. |
| `Discord not running` | Start the Discord desktop app. The app reconnects by itself. |
| Detected fine, but nothing on your profile | Turn on Discord → User Settings → Activity Privacy → "Share your detected activities with others". |

## Building

Install Rust from <https://rustup.rs> (the default MSVC toolchain), then:

```
cargo build --release
```

The exe ends up in `target\release\nnk-presence.exe`.

No Npcap SDK is needed: `wpcap.dll` is loaded at runtime from `C:\Windows\System32\Npcap`.

Tests run on small, anonymised captured frames:

```
cargo test
```

Replay a saved capture through the same pipeline (useful when the game updates and something breaks):

```
nnk-presence.exe --replay capture.pcapng
```

## Using your own Discord application

The release uses an application that already has the class images uploaded.
The app ID is set in `src/main.rs` (`APP_ID`). To use your own:

1. Create an application at <https://discord.com/developers/applications>. Its name is what Discord shows as "Playing …".
2. Under **Rich Presence → Art Assets**, upload one image per class with these keys:

   | Class     | Large image (portrait) | Small image (class icon) |
   |-----------|------------------------|--------------------------|
   | Swordsman | `class_swordsman`      | `icon_swordsman`         |
   | Engineer  | `class_engineer`       | `icon_engineer`          |
   | Witch     | `class_witch`          | `icon_witch`             |
   | Rogue     | `class_rogue`          | `icon_rogue`             |
   | Destroyer | `class_destroyer`      | `icon_destroyer`         |

   Missing keys simply show no image.

## How it works

The game client talks to the game server over TCP port 12000:

```
frame   = [length: u24 LE, includes the 4-byte header][opcode: u8][payload]
payload = plaintext XOR 84 20 d0 c9 78 b3 9a a7 (repeating, starting at payload byte 0)
```

The plaintext is made of packed little-endian structs; strings are `u16 length + UTF-8`.

| Direction | Opcode / shape | Used fields |
|---|---|---|
| server → client | `c1` character summary | `@9 u64 id`, `@19 u32 class`, `@23 u32 level`, last `u32` = CP |
| server → client | `c9` character info | `@3 u64 id`, `@23 u16 len` + name, then `u32 class` |
| server → client | `6d` world entry | `@3 u32 world`, `@7 u16 channel` |
| server → client | `69` / `6b` world change | `@11` / `@3 u32 world` |
| server → client | `e4` stat update | own id at `@1`; stat `0x013d` (CP) is the last entry |
| client → server | 9-byte payload, kind `00` | `@7 u16 channel` (channel switch) |
| client → server | 8-byte payload, kind `02` | `@7 u8 deck` (0-based) |

The opcode byte is not a reliable message type on its own (unrelated messages can share one), so every
handler also checks the shape: the character summary has a fixed 9-byte header and locks the own
character id, other character frames must carry that id, and world ids must exist in the game's
`World` table. Client-to-server opcodes change every time, so those messages are matched by shape only.

## Notes

- A game update can change offsets. If a field stops updating, record a short capture with
  Wireshark (`tcp port 12000`) and replay it with `--replay`.

## Disclaimer

Unofficial fan project, not affiliated with Netmarble or Level-5. It only reads your own network
traffic and never modifies the game client or its traffic. Use at your own risk.
Ni no Kuni: Cross Worlds names, the app icon and character art belong to their respective owners.

## License

MIT
