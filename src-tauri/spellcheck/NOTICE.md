# Local Korean spelling checker

The application runs a local Hunspell 1.7.3 engine with the Korean dictionary
`hunspell-dict-ko` 0.7.94. Text is checked on this computer; the checker does
not make network requests. The engine and dictionary are separate works with
separate license terms.

- Hunspell engine: https://github.com/hunspell/hunspell/releases/tag/v1.7.3
  (MPL 1.1 / GPL 2.0 / LGPL 2.1 choice, see the bundled
  `hunspell-1.7.3.tar.gz` for full copyright and license texts).
- Korean dictionary: https://github.com/spellcheck-ko/hunspell-dict-ko/releases/tag/0.7.94
  (`ko.aff` and `ko.dic` combined work: GPL 3.0 or later; see bundled
  `dictionary-LICENSE.md` and `dictionary-GPL-3.txt`). The generating sources
  include separately credited MPL/GPL/LGPL and Creative Commons data. The
  tagged source and its complete license files are included as
  `hunspell-dict-ko-0.7.94-source.tar.gz`.
- This application's integration runner source is `hunspell-runner.cpp`.
  `build-windows.bat` builds it with the included Hunspell source using
  Visual Studio 2022 C++ tools. These tools are needed only to rebuild the
  runner, not to use the installed application.

The selected binary was built for Windows x64. The dictionary and runner are
installed beside the app as Tauri resources, so the user does not need a
system Hunspell installation or a personal dictionary.
