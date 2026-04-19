# Egui Control Console Design

**Date:** 2026-04-18

**Goal:** Replace the current debug-style dashboard with a product-like control console that communicates service health, network state, and available actions without layout collapse.

## Context

The existing `egui` dashboard uses a simple grid of groups with no width constraints. In narrow or medium widths the cards collapse, labels wrap vertically, and the interface reads like a prototype instead of a desktop product. Quick actions are also visually weak and do not behave like primary navigation.

## Proposed Design

### Information Architecture

- Keep the current top menu for desktop conventions.
- Turn the main dashboard into a control console with four regions:
  - Hero header with service state and primary action
  - Stable metric cards for service, network, and clipboard
  - Device overview and operator tips in a right-hand column
  - Recent activity as a readable timeline instead of plain log lines

### Visual Direction

- Stay within `egui`, but stop relying on default spacing and default widget visuals.
- Use a deep blue-gray desktop palette with muted panels and saturated accent colors.
- Increase window size and spacing so the app feels deliberate instead of cramped.
- Use fixed-height cards and adaptive columns. Cards stack on narrow widths and render in rows on wider widths.

### Interaction Changes

- The primary start/stop action appears both in the top bar and in the hero section.
- Dashboard quick actions become real navigation buttons to `Devices`, `Layout`, and `Settings`.
- Device overview summarizes discovered and connected devices, then exposes a direct path to the device screen.

### Error Handling

- When the daemon is unavailable, the dashboard should still look intentional: status surfaces show offline state and the hero action invites the user to start the service.
- Empty activity and empty device states should use product copy rather than raw debug text.

### Testing

- Add unit tests for layout decisions so the dashboard switches between stacked and multi-column presentation predictably.
- Add tests for dashboard action labels so the hero action remains consistent with service state.

## Out of Scope

- Replacing `egui`
- Rebuilding the `Devices`, `Layout`, or `Settings` tabs from scratch
- Adding custom icons, external design assets, or animations
