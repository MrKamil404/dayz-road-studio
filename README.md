# Terrain Builder Road Merger — Rust

Aplikacja Windows do łączenia dróg z projektów `.tv4p`, z podglądem geometrii, filtrowaniem, zaznaczaniem i eksportem PNG. Aktualna aplikacja jest napisana w Rust. Stary skrypt Python pozostaje jako materiał referencyjny.

## Ostatnie zmiany

- Przepisanie aplikacji na Rust i odczyt kształtu części z modeli MLOD P3D.
- Listy i podglądy dróg dla A, B i wyniku, filtrowanie oraz zaznaczanie na liście i mapie.
- Eksport tylko zaznaczonych dróg, własna rozdzielczość PNG (w tym 15360 × 15360) i zapis w tle.
- Eksport na pełny obszar mapy z lewym dolnym rogiem (200000, 0), zachowujący współrzędne dróg.

## Uruchomienie

Uruchom `tv4p_merge_roads.exe`. Wczytaj projekt A i B, wskaż plik wynikowy i połącz drogi. Zakładki A, B i Wynik mają osobne listy, zaznaczenia i ustawienia widoku.

- Filtr tekstowy szuka po ID i nazwach części drogi. Dostępne są również filtr typu, zakres długości i widok tylko zaznaczonych.
- Kliknięcie wiersza lub drogi na mapie przełącza zaznaczenie. Zaznaczone drogi są żółte. Można zaznaczyć wszystkie widoczne lub wyczyścić zaznaczenie.
- Kółko myszy przybliża mapę, przeciąganie ją przesuwa. Przyciski dopasowania obejmują widoczne lub zaznaczone drogi.
- Eksport PNG obejmuje drogi widoczne po filtrowaniu albo **tylko zaznaczone**, również zaznaczone ukryte przez filtr. Przycisk pokazuje liczbę dróg w zakresie eksportu. Szerokość i wysokość PNG można wpisać niezależnie (kliknij pole liczby); dostępne jest przezroczyste tło.
- Domyślny eksport obejmuje **pełną mapę 15360 × 15360 m**, której lewy dolny róg ma współrzędne **E=200000, N=0**, i zapisuje obraz **15360 × 15360 px**. Rozmiar mapy, jej początek i rozdzielczość obrazu są osobnymi ustawieniami. Przy tych wartościach 1 piksel odpowiada 1 metrowi.
- Eksport pełnej mapy zachowuje położenie dróg: `x=(E−E0)×szerokośćPNG/szerokośćMapy`, `y=wysokośćPNG−(N−N0)×wysokośćPNG/wysokośćMapy`. Północ jest u góry, nie ma marginesu ani dopasowywania do zaznaczenia. Geometria poza granicami jest przycinana. Po wyłączeniu pełnego obszaru mapy kadr dopasowuje się do eksportowanych dróg.
- Dopuszczalne wymiary PNG to 128–32768 px na bok, maksymalnie 268 435 456 pikseli łącznie. Obraz 15360 × 15360 wymaga około 900 MiB na bufor RGBA oraz dodatkowej pamięci podczas zapisu. Eksport działa w tle i nie blokuje interfejsu.

## Geometria dróg

Domyślny folder modeli: `G:\dz\structures\roads\parts`. Można go zmienić w aplikacji. Program odczytuje siatkę najniższego wizualnego LOD z plików **MLOD P3D** i punkty pamięci `LB/PB`, `LE/PE`, `LH/LD`, `PH/PD`. Rzut siatki na płaszczyznę X/Z wyznacza rzeczywisty kształt, a punkty połączenia wyznaczają pozycję następnej części. Pliki ODOL nie są obsługiwane.

Każda droga zawiera część bazową, jej pozycję i obrót oraz osobne łańcuchy odgałęzień. Odczyt stosuje obrót z `0x8C` w stopniach (zgodnie z kierunkiem obrotu Terrain Buildera); `0x92` wychodzi z końca części bazowej, `0x93` z początku, `0x94` i `0x95` z bocznych połączeń. Lewy łuk jest łączony przez przeciwny koniec modelu. Interpretacja tych pól pochodzi z analizy danych TV4P i modeli, nie z opublikowanej specyfikacji formatu.

Gdy pliku modelu brakuje, program oblicza przebieg z nazwy: prosty odcinek z długości, łuk z kąta i promienia. Nazwy `6` i `12` oznaczają odpowiednio 6,25 m i 12,5 m; łuk `0 2000` oznacza 0,5°. Długość łuku to `promień × kąt w radianach`. Nieznane nazwy i brakujące połączenia są zgłaszane przy drodze; program nie zastępuje ich dowolną geometrią. Liczby części odczytanych z MLOD i z nazw widać w podpowiedzi wiersza. Długość obejmuje część bazową i wszystkie gałęzie.

## Łączenie projektów

Program sprawdza zgodność definicji typów i skrzyżowań (`0x88`, `0x89`), pomija identyczne drogi, nadaje dodanym elementom unikalne ID i przebudowuje blok dróg. Pozostałe dane pochodzą z A. Zmodyfikowana lub przesunięta droga może zostać dodana jako osobna droga. Plik wynikowy musi być inny niż oba wejścia. Wynik należy otworzyć w Terrain Builderze przed użyciem w projekcie mapy.

## Wiersz poleceń

```powershell
.\tv4p_merge_roads.exe merge A.tv4p B.tv4p wynik.tv4p
.\tv4p_merge_roads.exe png mapa.tv4p drogi.png
.\tv4p_merge_roads.exe export mapa.tv4p drogi.json
.\tv4p_merge_roads.exe roundtrip mapa.tv4p
.\tv4p_merge_roads.exe types mapa.tv4p
.\tv4p_merge_roads.exe inspect-p3d "G:\dz\structures\roads\parts\asf2_30 25.p3d"
```

Polecenia `png` i `export` przyjmują opcjonalny folder modeli jako ostatni argument. PNG z CLI ma rozmiar 2048 × 1536 i obejmuje wszystkie drogi.

## Budowanie

Wymagany Rust z narzędziami kompilacji Windows:

```powershell
cargo build --release
```

Gotowy program: `target\release\tv4p_merge_roads.exe`.
