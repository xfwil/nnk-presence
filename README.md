# nnk-presence

Discord Rich Presence for **Ni no Kuni: Cross Worlds** (PC).

A small Windows app that passively reads your own game traffic and shows your character on Discord:

```
Player · Engineer Lv 218
Sagartha · Ch. 3 · Deck 6
[class portrait]  hover: CP 10,216,846
```

It never touches the game process, never injects, and never sends anything to the game servers.
It only reads packets that already arrive on your network adapter, the same way Wireshark does.

## Requirements

- Windows 10/11
- [Npcap](https://npcap.com) installed. It comes with Wireshark, or you can install it on its own.
  If you choose "Restrict Npcap driver's access to Administrators only" during setup, run the app as Administrator.
- The Discord desktop app running on the same PC.

## Usage

1. Start `nnk-presence.exe`.
2. Start the game and log in. **Start the app before you log in**: your name, class, level and CP
   are only sent when you enter the game. If the app starts mid-session, it still picks up the
   map, channel and deck, but it waits for the next login (or character select → re-enter)
   before it shows anything on Discord.
3. The window shows what has been detected. Untick "Show on Discord" to hide the presence.

Errors are logged to `nnk-presence.log` next to the executable.

### What is shown

| Field   | Source                                                                    |
|---------|---------------------------------------------------------------------------|
| Name    | character info on login                                                   |
| Class   | character info, mapped with the game's `Class_Name` table                 |
| Level   | character summary on login                                                |
| CP      | on login, updated live when it changes                                    |
| Map     | updated on every map change, mapped with the game's `World_Name` table (English) |
| Channel | on map entry, and when you switch channel                                 |
| Deck    | when you switch deck (unknown until the first switch)                     |

## Building

```
cargo build --release
```

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

Opcodes are only stable from server to client. For client-to-server messages the opcode byte
changes every time, so they are recognised by payload shape instead.

## Notes

- If you run several Discord clients at once (Stable / PTB / Canary), the presence goes to whichever one started first.
- A game update can change offsets. If a field stops updating, record a short capture with
  Wireshark (`tcp port 12000`) and replay it with `--replay`.

## Disclaimer

Unofficial fan project, not affiliated with Netmarble or Level-5. It only reads your own network
traffic and never modifies the game client or its traffic. Use at your own risk.

## License

MIT
