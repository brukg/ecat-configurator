# EtherCAT Configurator

Desktop tool for building EtherCAT network configurations: it reads ESI device
descriptions (ETG.2000) and writes ENI network information files (ETG.2100).

## Features

- **Device catalog:** load ESI files or whole folders (several at once).
- **Documents:** every ENI or project file opens as its own document, and each
  document can be edited on its own.
- **Workspace:** dockable tabs, side-by-side or stacked splits, floating panels and
  separate windows. Each editor shows its own device and page.
- **Network:** topology with ports, station and auto-increment addresses, and
  identity checks.
- **Process data:** sync managers, PDO assignment and editable PDO mapping,
  and the process image with C header and CSV export.
- **Start-up settings:** CoE start-up SDOs (generated PDO assignment/mapping plus
  your own) and distributed clocks (operation modes, SYNC0/SYNC1 cycle and shift,
  reference clock).
- **ENI files:** generated EtherCAT init commands for every state transition,
  ENI export, and ENI import that round-trips.
- **EEPROM:** SII images generated from the ESI (`.bin` / Intel HEX).
- **Online:** live bus scan on Linux (raw socket), compared with the configuration.
- **Validation:** checks the configuration for problems such as address clashes,
  excluded PDOs, sizes and DC settings.

## Build and run

```sh
cargo build --release
./target/release/ecat-configurator [files…]
```

Files given on the command line (ENI, ESI or `.ecproj` projects) are opened at
start-up. You can also drop files onto the window.

Command-line use without a window:

```sh
ecat-configurator --esi devices/ --eni in.xml --export-eni out.xml --export-header pi.h
ecat-configurator --screenshot picture.png   # draws off-screen, no window is opened
```

`cargo test` runs the unit tests, which include an ENI export → import → export round trip.

## Debian package

```sh
packaging/build-deb.sh                      # writes dist/ecat-configurator_<version>_<arch>.deb
DEB_MAINTAINER="Name <mail>" packaging/build-deb.sh
```

The package installs the program, a menu entry, icons and the sample ESI files.
It gives the program the `cap_net_raw` capability so the bus scan works without
root. Dependencies are worked out from the build machine, so build on the oldest
distribution you want to support.

## Layout

| Path | Contents |
|------|----------|
| `src/esi.rs` | ESI model and parser |
| `src/model.rs` | network model, process image layout, init commands, validation |
| `src/eni.rs` | ENI writer and reader |
| `src/sii.rs` | SII / EEPROM image generator |
| `src/scan.rs` | raw-socket bus scan (Linux) |
| `src/export.rs` | C header and CSV export |
| `src/ui.rs` | user interface (documents, docking workspace, editors) |
| `src/offscreen.rs` | off-screen rendering for `--screenshot` |
| `samples/` | example ESI files built into the program |
| `packaging/` | Debian packaging |
