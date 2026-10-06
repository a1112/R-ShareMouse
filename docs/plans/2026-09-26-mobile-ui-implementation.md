# Mobile UI implementation plan

## Goal

Implement the approved discovery and landscape keyboard concept in the Android app and verify it on a local Android emulator.

## Approach

Keep discovery, pairing, and input transport in their existing Java/Rust owners. Rebuild the native discovery screen with a clear computer card and a secondary manual path. Restyle the authenticated web controller for landscape keyboard use and make the native mode switch reflect the selected mode.

## Steps

1. Add an Android UI test for the discovery screen hierarchy and the manual connection disclosure; confirm it fails against the current layout.
2. Implement the native discovery layout, computer card state, and mode header; make the UI test pass.
3. Add a small web keyboard structure/style check; confirm it fails, then update the web controller styles and markup to match the approved concept.
4. Build the APK and daemon, launch the local API 36 emulator, install the app, and inspect portrait and landscape screenshots. Exercise auto discovery, desktop approval, and mode switching without injecting actual keys into the computer. Repeat installation, discovery, pairing, scanner launch, and landscape checks on the Xiaomi 14 Pro when it is available.

## Visual reference

`docs/design/mobile-discovery-keyboard-concept.png`
