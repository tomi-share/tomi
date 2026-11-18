# grammar-check

Host test (Windows + Visual Studio Build Tools) of the PHP highlighter that the app ships in `libtomi_highlight.so`.
It builds `app/src/main/cpp/highlight_core.cpp` (the same code as the Android library) against the real grammars and prints a
file with every coloured run as `[type|text]`.

```powershell
# the grammar sources come from the Gradle cache: run `.\gradlew.bat :app:fetchGrammarSources` once
.\check.ps1 -Language php -File .\samples\sample.php
.\check.ps1 -Language php -File .\samples\sample.php -Tokens   # one run per line
```

Use it after any change to `app/src/main/cpp/queries/*.scm`: a query error (unknown node or token) is printed with its line and
column, and a wrong capture is visible in the output. Build outputs go to `build/` (ignored by git).

`samples/` holds one PHP file with the usual constructs (namespaces, attributes, enums, string interpolation,
heredoc, lambdas, annotations, accents and emoji to check the UTF-16 offsets).
