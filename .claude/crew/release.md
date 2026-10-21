# Wydawanie MyHUB

Wersja: SemVer, źródłem jest `package.json`. Workspace w głównym `Cargo.toml` musi mieć tę samą wersję (sprawdza `bun run version:check`). Aktualizacje pobiera aplikacja z `https://github.com/Ruuvak/MyHUB/releases/latest/download/latest.json`.

## Jednorazowa konfiguracja

1. W repozytorium GitHub: Settings, Secrets and variables, Actions, dodaj sekrety:
   - `TAURI_SIGNING_PRIVATE_KEY` = pełna zawartość pliku `C:\Users\creva\.tauri\myhub-updater.key`
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = puste
2. Repozytorium musi być publiczne, inaczej aplikacja nie pobierze `latest.json`.
3. Zrób kopię zapasową klucza prywatnego poza tym komputerem. Utrata klucza oznacza, że zainstalowane kopie nie przyjmą już żadnej aktualizacji (zmiana klucza wymaga ręcznej reinstalacji). Klucz publiczny jest w `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). Nigdy nie commituj klucza prywatnego.

## Wydanie

1. `bun scripts/version.ts set X.Y.Z`; w `CHANGELOG.md` zmień nagłówek `## X.Y.Z (unreleased)` na `## X.Y.Z` (workflow bierze z niego opis wydania)
2. `cargo check` (odświeża `Cargo.lock`)
3. Testy: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `bun run typecheck`
4. Commit zmian wersji.
5. `git tag vX.Y.Z` i `git push origin vX.Y.Z`. Tag musi zgadzać się z wersją, inaczej krok sprawdzenia w CI zatrzyma budowanie.
6. Workflow `release` buduje Windows (NSIS) i Linux (AppImage, deb) i tworzy wersję roboczą (draft) wydania z plikami i podpisami.
7. Pobierz i przetestuj instalatory z wersji roboczej.
8. Opublikuj draft. Dopiero wtedy `latest.json` staje się dostępny i zainstalowane kopie zobaczą aktualizację.

Ręczne uruchomienie (`gh workflow run release.yml --ref main`) tylko buduje, bez tagu i bez release. Paczki trafiają do artefaktów runu (`myhub-ubuntu-22.04`, `myhub-windows-latest`), pobierasz je przez `gh run download <id>`. Do testu na Kubuntu repo nie musi być publiczne.

## Lokalna budowa na Windows

W powłoce (Git Bash), z PATH z BRIEF.md:

```
export TAURI_SIGNING_PRIVATE_KEY="$(cat /c/Users/creva/.tauri/myhub-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
bun run tauri build --bundles nsis
```

Wynik: `target/release/bundle/nsis/MyHUB_X.Y.Z_x64-setup.exe` oraz plik `.sig` obok. Bez zmiennej `TAURI_SIGNING_PRIVATE_KEY` budowa kończy się błędem, bo włączone jest `createUpdaterArtifacts`.

## Linux

- Samo aktualizuje się tylko AppImage. Instalacje z `.deb` nie aktualizują się niezawodnie, wymieniaj je ręcznie.
- AppImage wymaga `chmod +x MyHUB_*.AppImage`.
- Dostęp do hidraw może wymagać reguły udev (nietestowane).

## Windows

- Instalacja per użytkownik, bez uprawnień administratora.
- Aktualizacja w trybie passive pokazuje pasek postępu instalatora i uruchamia aplikację ponownie.
- Przed instalacją aplikacja zatrzymuje obsługę urządzeń, więc mysz wraca do trybu onboard.
