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
   load check, and ENI round trip. ~~Several cyclic tasks with different cycle
   times~~ done in 0.14.
4. **Complete ESI support.** ~~Object dictionary~~ done in 0.4: dictionary
   page, start-up values and PDO entries picked by name, typed values, checks
   against access and size. ~~`AlternativeSmMapping` and external dictionary
   files~~ done in 0.15. Still open: multi-channel profiles (e.g. several CiA 402
   axes) and all InitCmd variants.
5. **Online commissioning.** ~~INIT/PREOP/BOOT, AL status codes, error counters,
   EEPROM read/write, SDO read/write~~ done in 0.10 (frame encoding unit-tested;
   not yet tried on real hardware here), ~~segmented SDO~~ done in 0.13. Still
   open: SAFEOP/OP with cyclic data, browsing the dictionary online (SDO info), DC
   delay measurement.
6. **Other mailbox protocols in the ENI.** ~~SoE start-up IDNs and EoE IP
   settings~~ done in 0.7, ~~mailbox state polling (`MailboxStates`)~~ done in
   0.11, ~~FoE firmware download~~ done in 0.12 (on the Online page). Still open:
   checking the EoE/SoE/FoE byte layout against real devices (see item 2).
7. ~~**Correct topology handling for junctions.**~~ Done in 0.6: bus order and
   auto-increment addresses from the port wiring (D, B, C), automatic re-sorting,
   port-conflict and order checks, and wiring rebuilt from a scan.
8. **Hot-connect groups and cable redundancy.** ~~Hot-connect groups~~ done in
   0.9 (group size, station alias identification, ENI HotConnect, checks, alias
   in the EEPROM image). Still open: cable redundancy (second master port).
9. ~~**Undo/redo and copy/paste.**~~ Done in 0.5: per-document undo history and
   cut/copy/paste/duplicate of slaves with all their settings (also between
   documents and program instances). Still open: copying only a PDO mapping or
   start-up list to several slaves at once.
10. **Cross-platform.** ~~Windows and macOS builds, and CI that builds, tests
    and packages every commit~~ done in 0.8, ~~macOS app bundle and Windows zip
    from CI~~ done in 0.16. Still open: signed installers (MSI, notarized DMG) and
    a bus scan via Npcap (Windows) and BPF (macOS).
