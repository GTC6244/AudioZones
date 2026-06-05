# Changelog

All notable changes to AudioZones are recorded here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- **Create zones from the app.** `POST /zones` builds a new (inactive) zone from a name
  and one or more links. A "New zone" editor (FAB on the Zones tab + empty-state button)
  assembles the links; the destination picker is filtered to input ports containing
  "playback" (the sink inputs audio routes into). A created zone with links auto-derives
  its volume node, so its tile gets a working volume slider immediately.
- **Edit zones.** `PUT /zones/:name` renames a zone and/or adds/removes its links, while
  preserving its volume settings. An active zone stays active under its new name. Reached
  via the new "Edit zone" tile menu item, sharing the create editor.
- **Delete zones.** `DELETE /zones/:name` removes a zone (and drops it from the active
  set), behind a confirmation dialog on each tile.
- `ZoneView.links` on the wire — the zone's defined routing recipe — so the editor can
  prefill without a separate fetch.

### Changed
- The Zones empty state now offers a "Create zone" button instead of pointing users at
  `zones.toml` on the server.
- `ZoneStore` enforces unique, non-empty zone names; the create/edit/delete handlers map
  conflicts to `409`, bad input to `400`, and unknown zones to `404`.

### Fixed
- Added the `INTERNET` permission to the Android **release** manifest. It was only present
  in the debug manifest, so sideloaded release builds couldn't reach the server.
