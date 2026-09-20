# Provenance

This project is a clean-room rewrite. Its licence (MIT OR Apache-2.0) is only meaningful if the code here
was written from behaviour and documentation rather than translated from the GPL-2.0 project it replaces.

This file records, per module, what each was written from. It is an audit trail, not decoration.

## Rules

**Written from**  -  the observable behaviour of the hardware and existing programs, the public USB HID and
Logitech LCD documentation, and *this project's own* documentation, applet format, source-kind specification
and tests (all written here, all unencumbered).

**Never**  -  code, identifiers, comments, structure or file organisation copied from the C++ driver or the
Java tool of `npc-nathan/linux-g13-driver`; any vendored or pasted fragment of it.

Where a behaviour could only be established by watching the old programs run, that is recorded below as an
*observation*, with what was observed and how.

## Record

| module | written from | observation recorded |
|---|---|---|
| `g13-proto`  -  `descriptor` | the USB HID specification, §6.2.2 (item format, item types, global/local/main state, and that a main item clears the local state) |  -  |
| `g13-proto`  -  `report` | the device's own report descriptor, quoted below | report 0x01 is one input item, usage page 0xff00, size 8 × count 7, variable (a bitmap, not an array of usage codes); the 7 payload bytes follow a report id byte, so the wire packet is 8 |
| `g13-proto`  -  `keymap` | the walk on the real pad | every control and bit measured with `g13 watch --map` on 2026-09-15: G1-G22 are bits 16-37, LR 40, L1-L4 41-44, M1-M3 45-47, MR 48, LMB 49, RMB 50, JCLICK 51. LR was re-pressed on its own to confirm bit 40 after the first reading was ambiguous. Bits 39 and 55 are device flags; 38 and 52-54 never responded. The stick spans bits 0-15 |
| `g13-proto`  -  tests | the descriptor bytes returned by the real pad | `tests/fixtures/g13-report-descriptor.bin`, 61 bytes, taken with `cargo run -p g13-device --example descriptor` on 2026-09-15; asserts the 56-bit key report, the 991-byte LCD output report, and the four feature reports |
| `g13-device`  -  `keyboard` | the kernel's uinput interface, via the evdev crate's documented builder | the virtual device is named `g13 keyboard`  -  this project's own naming, not the predecessor's  -  and advertises the full key range so a binding can ask for any key |
| `g13-device` | libusb's API documentation | the interface is HID class with interrupt endpoints 0x81 in (8 bytes) and 0x02 out (64 bytes); a `GET_DESCRIPTOR(Report)` on a *claimed* interface returns EPIPE, so the descriptor was read in a window when the previous driver's service was stopped |
| `g13-config` | our own formats, documented in `docs/bindings.md` and `docs/macros.md`; the previous driver's vocabulary is read so that a user's existing file keeps working, never to decide what the pad has | `bindings-N.properties` and `macro-<id>.properties` read and written round-trip, byte for byte |
| `g13-screen` | the device's own framebuffer layout, from the output report in the descriptor, and this project's widget spec in `docs/applets-and-sources.md` | one byte is a column of eight rows with bit 0 at the top, and a frame is 991 bytes with blanking rows - both seen on the pad |
| `g13-sources` | this project's own source-kind specification (`<kind>:<rest>#<path>`), a page per kind in `docs/applets-and-sources.md` | what a `json:` or `file:` source does while its file is being written, and how far an endpoint's token travels |
| `g13-applets` | this project's own applet schema and widget list, documented in `docs/applets-and-sources.md` | the widget kinds the previous stack's applets used, re-drawn from how they look rather than copied |
| `g13-agent` | this project's own architecture: the interfaces `g13-device`, `g13-screen`, `g13-sources` and `g13-applets` expose | the input loop never waits for a source - measured on the pad, which is why gathering and drawing are two threads |
| `g13-cli` | our own tool behaviour; the previous stack's eleven entry points are the list of jobs to replace, not a source of code | `g13 doctor`'s report and `g13 watch`'s output, both chosen here |
| `g13-gui` | egui's own documentation, and the window's design system as written here - one shell, one table shape, one field grid | the window's rates: a preview beside a moving applet repaints at 50 ms, from the same rule the driver uses |

## Control identities are not inherited

No module derives the identity, name, existence or count of this device's controls from the predecessor's
configuration files or source code. What the pad has is established by observing the pad: a vendor-defined
report of 56 bits, walked positionally, with the bits that answer recorded against the press that produced
them. A control this program has not seen pressed is reported as not pressed, never as absent.

The bindings reader in `g13-config` exists so that a user's existing configuration keeps working, which is
PROVENANCE-independent compatibility work. It carries no claim about hardware and must never be used to
decide what the pad has.

**And the predecessor's key names are not translated.** A bindings file is a key map  -  it says what each
pad control sends, and it belongs to its user. Copying or reverse-engineering the predecessor's names for
its keys in order to transplant that map is not compatibility work; it is inheritance by another route. The
new build keeps the file format and reads and writes it, with the controls named as this project measured
them, and the map itself is whatever its owner sets.

## The font

The panel's font is a bitmap: one byte a column, eight rows, the top row in bit 0. Three tables now.

| table | written from |
|---|---|
| `GLYPHS`  -  the 128 ASCII glyphs | this project's own, drawn for the 160x43 panel |
| `GREEK`  -  56 glyphs | `font8x8` (Daniel Hepper, **Public Domain**), which derives from Marcel Sondaar's and IBM's public-domain VGA fonts. Transposed from rows to columns here, and its seven columns of ink trimmed to six to fit the advance |
| `CYRILLIC`  -  130 glyphs | `CyrAsia-Terminus12x6.psf.gz` from `console-setup`, whose own copyright file states *"All console fonts are public domain by nature."* Terminus is by Dimitar Zhekov, public domain. Transposed, and the ink window rows 2-9 of its twelve taken to fit eight |

**Not copied, converted.** Both sources store a byte a *row* with the leftmost pixel in the high bit; this font
stores a byte a *column* with the top row in bit 0, so every glyph is transposed. **Verified by rendering the
converted glyphs as text and reading them** - Alpha, Theta, Omega, Psi, А, Б, Д, Ж, Я and their lower-case forms -
rather than by trusting the conversion.

**Known and accepted:** the Cyrillic source's letters are twelve rows tall and this font has eight, so the descenders
of `Д`, `Ц` and `Щ` are trimmed. They stay legible; they are not exact. A taller source would fix it and would change
the panel's line height with it.

## Still to be established by observation, not copying

The descriptor calls the 56 key bits **vendor-defined** and does not name them, so which control sets
which bit cannot be read from anywhere  -  it is measured. `g13 watch --map` walks the pad, records the bits
each control sets, and prints the table; the discrete controls are now mapped in `g13-proto::keymap`.

**Still open:** the stick spans bits 0-15 as two bytes, and what those bytes encode is not established.
They may be positions, or a bitfield that includes flag bits. Nothing treats them as axes until using the
stick shows what they are.
