# Plan: zapis DPI i report rate do profilu onboard (0x8100)

Status: propozycja, nic nie wykonane. Każdy krok na urządzeniu wymaga osobnej zgody w czacie.

## Dlaczego
Test z 2026-10-15 pokazał, że w trybie HOST mysz wyłącza przyciski onboard (DPI +/-, shift, G-Shift, profil, back/forward) i diody. Zapis do profilu onboard zostawia mysz w trybie ONBOARD, więc wszystkie te funkcje dalej działają.

## Fakty, na których plan stoi
- [OBS] Sekwencja zapisu (REA `notes/hidpp_spec.md:35-43, 149-156`, `ghidra/out/write_sector.c`): fn6 BeginWrite(sektor BE16, offset 0, długość 255), 16 ramek fn7 WriteData po 16 B (ostatnia dopełniona), fn8 EndWrite zawsze, także po błędzie. G HUB zapisuje zawsze cały sektor, bez ponowień i bez odczytu zwrotnego.
- [OBS] Obraz sektora: 255 B, CRC-16/CCITT-FALSE BE w bajtach 0xFD..0xFE. Kod ma już CRC (`crates/hidpp/src/crc.rs`) i parser (`onboard.rs`), brak enkodera.
- [OBS] Layout A: 0x00 rate (interwał w ms), 0x01 domyślny indeks DPI, 0x02 indeks shift, 0x03 pięć poziomów DPI (LE u16).
- [OBS] Sektory 0x0003..0x0005 są wyłączone w katalogu 0x0000, mają poprawne CRC i identyczną zawartość (1000 Hz, DPI 800/1200/1600/2400/3200).
- [UNK] Odpowiedź fn8 i kody błędów fn6/fn7. Czy mysz sprawdza CRC przy zapisie i co robi przy złym CRC. Czy po zapisie aktywnego profilu przeładowuje go od razu. Czy przycisk zmiany profilu pomija wyłączone sektory. Zachowanie licznika zapisów (0x12). Wytrzymałość flash.

## Wyniki etapu 1 (libratbag, Solaar, 2026-10-15)
- [OBS] Oba narzędzia: fn6 (sektor BE16, offset 0, długość = rozmiar sektora), fn7 po 16 B, fn8 bez parametrów; nie czytają bajtów odpowiedzi poza kodem błędu. Zawsze cały sektor. CRC liczone przez host, BE na końcu. Katalog 0x0000: wpisy 4 B `00 nn ee 00` (ee = włączony), potem `FF FF`, `00 00`, reszta 0xFF. Brak fn3 ani przeładowania po zapisie.
- [OBS] libratbag nie wysyła fn8 po błędzie fn7; G HUB wysyła. Decyzja: idziemy za G HUB (producent), fn8 zawsze.
- [OBS] Solaar wysyła ostatni kawałek 15 B w raporcie 20 B, więc bajt dopełnienia to 0x00 z ramki; G HUB dopełnia z szablonu [UNK]. Decyzja: dopełnienie 0x00.
- [OBS] Solaar pomija zapis, gdy obraz jest identyczny z odczytanym. Aplikacja ma robić to samo (zużycie flash); test etapu 3 celowo zapisuje identyczny obraz.
- [OBS] libratbag numeruje formaty profili: 3 = typ G900, 5 = typ G502X. Nasza mysz zgłasza 3, a układ pól jest potwierdzony na fixture (CRC 6/6, sloty), więc zostaje sprawdzenie format == 3.
- [UNK] Reakcja myszy na złe CRC, przeładowanie aktywnego profilu po zapisie, pomijanie wyłączonych profili przez przycisk, licznik 0x12, wytrzymałość flash. Do sprawdzenia w testach sprzętowych.

## Ważne ograniczenie
Sam zapis do wyłączonego sektora 0x0003 nie zmienia zachowania myszy. Żeby nowe DPI i rate zaczęły działać, sektor trzeba udostępnić: albo włączyć go w katalogu 0x0000 (drugi zapis flash), albo zapisywać do aktywnego profilu 0x0002. To decyzja na etap 5, po testach na wyłączonym sektorze.

## Etapy

### Etap 0: decyzja (Ty)
Zmiana decyzji "route a" w BRIEF: dopuszczamy zapis flash, wyłącznie DPI, indeksy DPI i rate, wyłącznie w sektorach profili użytkownika. Przypisania przycisków dalej tylko do odczytu.

### Etap 1: weryfikacja niewiadomych (bez urządzenia)
Researcher sprawdza implementacje zapisu w libratbag (hidpp20 onboard profiles) i Solaar: kolejność fn6/fn7/fn8, odpowiedź fn8, kody błędów, obsługę CRC, przeładowanie profilu po zapisie. To otwarte źródła, więc nie dotykamy plików Logitecha. Każde [INF] potwierdzone albo zostaje jako ryzyko w etapie 3.

### Etap 2: kod (bez urządzenia, tylko fixture)
1. `crates/hidpp`: funkcja nakładająca pola (rate, indeksy, 5 poziomów DPI) na surowe bajty odczytane z myszy; reszta bajtów bez zmian; przeliczenie CRC. Odmowa, gdy format ID != 3, gdy wartości są poza zakresem z 0x2201/0x8060 albo gdy indeks DPI > 4.
2. Polityka: nowy tier `onboard_write` (fn6, fn7, fn8), dostępny tylko przez osobną bramkę w sesji. fn6 przyjmuje wyłącznie offset 0 i długość 255 oraz sektor z listy dozwolonych. Na start lista to tylko 0x0003..0x0005. 0x0000, 0x0001, 0x0002 dochodzą dopiero po decyzji z etapu 5. 0x0101..0x0105 i wszystko inne: twarda odmowa przed wysłaniem bajtu. fn3 dalej zablokowany.
3. Transport: `write_sector(sektor, obraz)`: kopia zapasowa sektora do pliku (odmowa nadpisania istniejącej), fn6, 16 x fn7 z kontrolą statusu, fn8 zawsze, potem odczyt fn5 całego sektora, porównanie bajt w bajt i kontrola CRC. Przy niezgodności: jedna próba zapisu kopii zapasowej z tą samą weryfikacją, potem stop i raport.
4. `hw_probe`: komendy `backup <plik>` (sektory 0x0000..0x0005), `write-same <sektor>` (zapis identycznego obrazu), `write-profile <sektor> --dpi a,b,c,d,e --default N --shift N --rate HZ`, `restore <sektor> <plik>`.
5. Testy na fixture: zapis i odczyt obrazu, CRC, niezmienione bajty, odmowy polityki (ROM, zły offset, zła długość, sektor spoza listy), sweep allowlisty.
6. Reviewer obowiązkowo (allowlista, ścieżka zapisu, transport).

### Etap 3: test sprzętowy 1, zapis bez zmiany treści (zgoda)
Mysz w ONBOARD. Kopia sektorów 0x0000..0x0005, potem zapis 0x0003 dokładnie tymi samymi bajtami, które ma. Odczyt zwrotny, CRC, porównanie wszystkich 8 sektorów z kopią (powinny być identyczne). Sprawdza samą sekwencję przy zerowym ryzyku zmiany treści. Notujemy odpowiedź fn8 i licznik 0x12.

### Etap 4: test sprzętowy 2, zmieniona treść w wyłączonym sektorze (zgoda)
Zapis 0x0003 z innymi DPI (np. 400/800/1200/1600/3200) i rate 500 Hz. Odczyt zwrotny i CRC. Wyłączenie i włączenie myszy, ponowny odczyt (trwałość). Zachowanie myszy nie powinno się zmienić, bo sektor jest wyłączony. Na koniec opcjonalnie `restore` do oryginału.

### Etap 5: udostępnienie profilu (decyzja Ty, potem zgoda na test)
- Wariant A (proponowany): 0x0003 jako "profil MyHUB". Włączenie go w katalogu 0x0000 (zapis katalogu z tą samą procedurą), wybór przyciskiem profilu albo przez fn3 (wymaga dopisania fn3 do allowlisty i review). Twój profil 0x0002 zostaje nietknięty jako punkt powrotu.
- Wariant B: zapis bezpośrednio do aktywnego 0x0002. Prościej, bez katalogu i fn3, ale zmieniasz swój główny profil. Nie wiadomo, czy mysz przeładuje go bez restartu.

### Etap 6: aplikacja
DPI i rate zapisywane na mysz tylko po kliknięciu "Zapisz na mysz", nigdy przy przesuwaniu suwaka (zużycie flash). Licznik zapisów w ustawieniach. Kopie zapasowe sektorów w katalogu danych aplikacji przed każdym zapisem. Tryb HOST zostaje jako opcja albo znika, do decyzji.

## Odzyskiwanie
- Kopie sektorów z każdego zapisu plus `before.json` z testu 2026-10-15.
- `restore` zapisuje kopię tą samą zweryfikowaną ścieżką.
- Profile ROM 0x0101..0x0105 nigdy nie są zapisywane, więc mysz ma fabryczny punkt odniesienia [INF: zachowanie przy uszkodzonym profilu użytkownika niepotwierdzone].
- Nic z tego nie dotyka firmware ani DFU.
