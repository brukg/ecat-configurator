# Roadmap: missing critical features

Ordered by how much each gap blocks real projects. The first items decide
whether a generated ENI works on real hardware at all.

1. ~~**Modular devices (ESI Modules / Slots).**~~ Done in 0.2: slot
   configuration, per-slot PDO/object shifts, module list (0xF030) and module
   start-up parameters, validation, and ENI round trip. Still open: ESI
   `SlotGroup`s and `ModulePdoGroup` alignment.
2. **ENI conformance with real masters.** Validate output against the ETG.2100
   XSD and test it with common masters (e.g. SOEM, IgH EtherLab, commercial
   stacks). Add per-master export profiles where they differ (logical address
   layout, init-command set, DC fields).
3. ~~**Large process images.**~~ Done in 0.3: process data split over several
   frames and commands, with per-command working counters, wire time and bus
   load check, and ENI round trip. Still open: several cyclic tasks with
   different cycle times.
4. **Complete ESI support.** ~~Object dictionary~~ done in 0.4: dictionary
   page, start-up values and PDO entries picked by name, typed values, checks
   against access and size. Still open: `AlternativeSmMapping`, multi-channel
   profiles (e.g. several CiA 402 axes), external dictionary files and all
   InitCmd variants.
5. **Online commissioning.** Switch slave states (INIT/PREOP/SAFEOP/OP), browse
   and edit the object dictionary over SDO, read and write slave EEPROM, show AL
   status codes and error counters, and measure DC propagation delays.
6. **Other mailbox protocols in the ENI.** SoE start-up IDNs for servo drives, EoE
   IP/MAC settings, FoE firmware download, and mailbox polling configuration
   (`MailboxStates`).
7. **Correct topology handling for junctions.** Derive the bus order and the
   auto-increment addresses from port wiring (port order D, B, C) instead of the
   list order. Import topology from a scan.
8. **Hot-connect groups and cable redundancy.**
9. **Undo/redo and copy/paste** of slaves, PDO mappings and start-up commands.
10. **Cross-platform.** Windows and macOS builds and packages, and a bus scan via
    Npcap/BPF. CI that builds, tests and packages every commit.
