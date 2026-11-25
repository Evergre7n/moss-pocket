# Asset notices

## MOSS branding

The MOSS logo and wordmarks identify MOSS Pocket. Rights in the MOSS name and marks are reserved. The official source and Windows releases include the following branding files from the MOSS project.

- `relay-icon.png` and `relay-icon.ico` supply the app and executable icons.
- `relay-wordmark.png` and `relay-wordmark-light.png` supply the wordmark for the dark and light themes.

## README images

The README cover and in-app image viewer use a photograph of living moss by Engin Akyurt, taken on February 5, 2025. The documentation crops and resizes the photograph for display. The photograph retains its [Pexels license](https://www.pexels.com/license/). [Original photograph](https://www.pexels.com/photo/close-up-of-vibrant-green-moss-with-sporophytes-33212365/)

The app screenshots show sample notes in the Chinese and English interfaces. The MOSS marks retain the rights described above.

## Embedded fonts

The Windows build embeds the following files from [`epaint_default_fonts` 0.37.0](https://crates.io/crates/epaint_default_fonts/0.37.0). Each font keeps its upstream license.

- **Hack Regular** (`Hack-Regular.ttf`). Copyright 2018 Source Foundry Authors and copyright 2003 Bitstream, Inc. [Hack, DejaVu and Bitstream Vera notices](licenses/assets/Hack-Regular.txt)
- **Ubuntu Light** (`Ubuntu-Light.ttf`). Copyright 2011 Canonical Ltd. [Ubuntu Font Licence 1.0](licenses/assets/UFL.txt)
- **egui icon subset** (`egui-icons.ttf`). Copyright 2014 John Slegers. [Emoji Icon Font MIT License](licenses/assets/emoji-icon-font-mit-license.txt)

The app also loads fonts already installed on the user's system. Windows font files stay on the user's device. [Microsoft Windows font redistribution FAQ](https://learn.microsoft.com/en-us/typography/fonts/font-faq)

## Audio test fixtures

The source package's AAC, WMA and Opus test clips in `crates/mp-app/tests/fixtures/` are short synthetic 440 Hz tones encoded with FFmpeg. The test suite reads these files when testing audio decoding and seeking. The portable executable ships independently of the fixtures and FFmpeg.
