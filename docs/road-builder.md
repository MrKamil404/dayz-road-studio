# Road Builder — DayZ Road Tools

Natywny edytor dróg w Rust dla projektów Terrain Builder `.tv4p`. Interfejs Polski / English, oparty na eframe/egui. Bazuje na odczycie TV4P i geometrii MLOD z [DayZRoadsMerge / Terrain Builder Road Merger](https://github.com/MrKamil404/Terrain-Builder-Road-Merger). Oryginalny plik TV4P nie jest nadpisywany przez tę aplikację.

Ten toolset jest zintegrowany z Merge w jednym programie. Uruchom `tv4p_merge_roads.exe` i wybierz **Road Builder** w launcherze. Zakładki u góry przełączają narzędzia bez utraty stanu; wybór języka jest wspólny. Przełączenie nie przenosi projektów między narzędziami. Poniżej zachowano opis funkcji oryginalnego DayZRoadToolExternal 0.1.5.

Wersja **0.1.1** poprawia eksport nowych dróg: pole `0x8F` odwołuje się do pozycji typu drogi w liście `0x88`, a nie do indeksu segmentu. Przed zapisem nowe rekordy są dodatkowo sprawdzane względem definicji Road Tool: typ, kategoria, model i indeksy segmentów. Projekty `.dzroad` z wersji 0.1 można ponownie otworzyć i wyeksportować bez ponownego rysowania; wcześniejsze eksporty TV4P należy wygenerować ponownie.

## Uruchomienie

Wersja **0.1.5** dodaje **Dopasuj ASC pod wybraną drogą** oraz **Eksport ASC…** w prawym panelu. Wczytaj ASC, wybierz trasę projektu z wygenerowanymi segmentami i zakończ rysowanie. Modyfikacja obejmuje wyłącznie komórki, których środki leżą wewnątrz rzeczywistych trójkątów modeli MLOD po ich obrocie i przesunięciu. Uwzględnia zakręty, odwrócone party i `konec`; nie używa bufora wokół linii ani stałej szerokości drogi. Przy zbyt grubym rastrze pod wąskim partem może nie być środka żadnej komórki.

Profil podłużny jest wyznaczany z wysokości bieżącego ASC na portach łączących party. Monotoniczna interpolacja Hermite'a zapewnia wspólną wysokość i nachylenie na łączeniach, bez przeregulowania między wysokościami portów. Cała szerokość parta otrzymuje wysokość tego profilu: poprzeczne nierówności są usuwane. Przerwy w geometrii i komórki NODATA pozostają niezmienione. Poza obrysem modeli wysokości pozostają zachowane; nie ma dodatkowego wygładzania poboczy. Brak wysokości na wymaganym porcie lub niepołączone party przerywają operację.

Operacja tworzy nowy cache ASC, aktualizuje poziomice i profil w programie, a Ctrl+Z / Ctrl+Y przywraca poprzednią / zmienioną wersję. **Zapisz projekt** zachowuje odnośnik do zmodyfikowanego cache. **Eksport ASC…** zapisuje teren z aktualnym położeniem i rozdzielczością do osobnego pliku do importu w Terrain Builder. Źródłowy ASC nie jest nadpisywany. Zmiany wysokości nie są zapisywane przez eksport TV4P. Ta funkcja działa dla tras projektu z wygenerowanymi partami; samo wskazanie istniejącej drogi bazowego TV4P nie udostępnia operacji.

Wersja **0.1.4** dodaje tryb **Na żywo**: pierwszy klik wskazuje początek, ruch kursora wylicza rzeczywiste segmenty i pokazuje je na zielono, kolejny klik zatwierdza dopasowanie, a Enter zapisuje gotową drogę. Wygenerowane zakończenia `konec` przesuwają się wraz z końcem trasy. Obliczenia działają w tle, uwzględniają bieżące ograniczenia terenu i obszary zakazane. Czerwony przebieg oznacza, że dopasowanie się nie powiodło; taki punkt nie jest zatwierdzany. Podgląd obejmuje dopasowanie całej rysowanej trasy i może zmieniać wcześniejsze segmenty. Ten tryb dopasowuje modele do wskazanego przebiegu; wyszukiwanie A* pozostaje dostępne osobnym przyciskiem.

**Import dróg SHP…** wczytuje PolyLine, PolyLineZ i PolyLineM. Każda część rekordu wieloczęściowego tworzy osobną trasę. W oknie importu wybierz typ drogi, współrzędne świata lub przesunięcie o początek mapy, ewentualnie własne przesunięcie E/N. Po imporcie wybierz trasę i **Dopasuj modele do punktów**, aby otrzymać segmenty do eksportu TV4P. Cały import można cofnąć jednym Ctrl+Z. Punkty zapisują się w projekcie `.dzroad`.

SHP musi mieć współrzędne w metrach i w układzie mapy. Import nie przelicza układów współrzędnych; zawartość opcjonalnego `.prj` jest pokazana do sprawdzenia. Wartości Z/M oraz atrybuty DBF nie są używane, a wysokości pochodzą z ASC. SHX i DBF nie są wymagane. Odczyt działa rekordami i sprawdza rozmiary, indeksy części oraz współrzędne. Obsługiwane struktury opisuje [specyfikacja ESRI Shapefile](https://www.esri.com/content/dam/esrisites/sitecore-archive/Files/Pdfs/library/whitepapers/pdfs/shapefile.pdf).

Wersja **0.1.3** podczas **Dopasuj modele do punktów** oraz **Wyznacz trasę po terenie** umieszcza segment `konec` danego typu na początku i na końcu drogi. Zakończenia są częścią dopasowania: pozostają w tolerancji punktów trasy, łączą się portami bez szczelin i podlegają ograniczeniom terenu. Brak `konec` w definicjach Road Tool lub brak jego modelu MLOD powoduje komunikat. Istniejące trasy należy ponownie wygenerować, aby otrzymały zakończenia; samo otwarcie projektu ich nie zmienia. Tryb ręcznego układania segmentów pozostaje ręczny.

Wersja **0.1.2** poprawia kierunki zakrętów w TV4P. Wstawiony zakręt używa kodu 7 (odwrócony) lub 8 (zgodny z kierunkiem modelu), a nie kodu 4 oznaczającego kategorię modelu w katalogu edytora. Walidacja odrzuca teraz kod 4 w rekordach wstawionych segmentów. Punkty i dopasowane segmenty istniejących `.dzroad` można zachować i ponownie wyeksportować.

```powershell
cargo run --release
```

Gotowy plik po kompilacji: `target/release/tv4p_merge_roads.exe`. Budowanie: `cargo build --release`. Testy: `cargo test --workspace`. Kontrola kodu: `cargo clippy --workspace --all-targets -- -D warnings`.

## Praca z edytorem

1. **Otwórz TV4P** — bazowy projekt z definicjami Road Tool. **Folder modeli MLOD** wskazuje niezbinaryzowane modele `.p3d`. Domyślnie `P:\dz\structures\roads\parts`. Katalog korzysta z definicji projektu, nie z dowolnych plików w folderze. ODOL nie jest obsługiwany przez edytor.
2. Wczytaj **Satelitę BMP / PNG** i opcjonalnie **Wysokości ASC**. Ustaw zasięg mapy w metrach. Domyślny lewy dolny róg: E=200000, N=0; północ na górze. Przycisk **1 piksel = 1 metr** ustawia wymiary mapy z wymiarów satelity. Wymiary można zmienić niezależnie.
3. **Rysuj**: klikaj punkty przebiegu, zakończ Enterem lub przyciskiem. Wybierz trasę na liście i **Dopasuj modele do punktów**. Narożniki są zaokrąglane przy ustawionym minimalnym promieniu. Punkty zbyt bliskie lub niedopasowywalne modele powodują komunikat; poprzednia droga pozostaje zachowana.
4. **Segmenty**: wybierz konkretny model i kierunek startu (90° oznacza północ). Kliknięcie tworzy nową drogę. Przycisk **Dodaj segment** przedłuża wybraną drogę, łącząc porty modeli bez szczelin. Odwracanie dotyczy zakrętów poza pierwszym segmentem. Można usuwać ostatni segment.
5. **Wybierz**: kliknij istniejącą drogę lub punkt trasy. Przeciąganie drogi przesuwa ją; przeciąganie punktu zmienia przebieg i usuwa dotychczasowe dopasowanie segmentów. Istniejące drogi TV4P można przesuwać, obracać i usuwać; ich odgałęzienia pozostają zachowane.
6. **Zakaz**: narysuj wielokąt i zakończ Enterem. Automat i dopasowanie omijają obszary zakazane. Można usunąć ostatni obszar.
7. **Wyznacz trasę po terenie**: zaznacz trasę z co najmniej dwoma punktami. A* szuka połączenia przez punkty pośrednie, uwzględniając nachylenie i obszary zakazane. Następnie wyznaczony przebieg jest zaokrąglany i składany z modeli. Wynik jest stosowany tylko, jeśli oba etapy się powiodą. Parametry kroku, dopuszczalnego spadku, preferencji łagodnego terenu, promienia i tolerancji są edytowalne. Brak danych ASC jest przeszkodą.
8. **Zapisz projekt** zapisuje `.dzroad`: punkty, segmenty, położenie mapy i ASC, ustawienia, zmiany istniejących dróg oraz odnośniki do warstw i cache. **Eksport TV4P** zapisuje inny plik niż wejściowy. Po eksporcie otwórz wynik w Terrain Builder i sprawdź działanie Road Tool.

Kółko myszy: zoom wokół kursora. Środkowy lub prawy przycisk: przesuwanie mapy. Ctrl+Z / Ctrl+Y: cofanie / ponawianie operacji geometrii. Escape: anulowanie rysowania. Po zamknięciu aplikacja pyta o niezapisane zmiany.

## Duże rastry i ASC

- PNG (także paletowy, 16-bitowy i Adam7) jest dekodowany wierszami do RGBA na dysku. BMP: nieskompresowane 8-bitowe z paletą, 24-bitowe i 32-bitowe, obie orientacje wierszy. Inne odmiany BMP wymagają konwersji do PNG; aplikacja zgłasza to przed importem.
- Piramida poziomów powiększenia jest zapisywana jako pliki RGBA. RAM importu rośnie z szerokością wiersza, a nie z liczbą pikseli całego obrazu. Widok korzysta z kafelków 256×256, z budżetem 256 tekstur (około 64 MiB RGBA). Nie ma stałego limitu wymiarów obrazu narzuconego przez aplikację. Limity systemu, pamięć potrzebna na wiersz i wolne miejsce na dysku nadal obowiązują.
- Cache satelity wymaga około `szerokość × wysokość × 4 × 4/3` bajtów. Przykładowo 15360²: około 1.26 GB (1.17 GiB). Pierwszy import może potrwać; zadania działają w tle i można je anulować.
- ASC jest parsowany strumieniowo do 4 bajtów na komórkę, następnie mapowany do pamięci wirtualnej. Obsługiwane `xllcorner/yllcorner` i `xllcenter/yllcenter`, `NODATA_value`, dane rozbite na dowolne wiersze. Poziomice są wyliczane w widocznym obszarze, z rozdzielczością zależną od zoomu. Profil i routing odczytują oryginalne wysokości z interpolacją biliniową.
- ASC zachowuje współrzędne nagłówka. Możesz przesunąć jego początek lub wyrównać go do początku mapy. Nie ma automatycznej reprojekcji układów współrzędnych.
- `.road-cache` powstaje w katalogu roboczym aplikacji. Zapisany projekt używa istniejącego cache przy ponownym otwarciu. Plik `.dzroad` nie zawiera satelity ani ASC; przy przenoszeniu projektu zachowaj źródła lub odpowiednie cache. Nie zmieniaj plików cache podczas działania aplikacji. Usuwanie niepotrzebnych cache wykonuj po zamknięciu aplikacji; ponowny import odtworzy brakujące warstwy ze źródeł.

## Granice wersji 0.1

- Nowe skrzyżowania, projektowanie całej sieci, mosty i tunele nie są obsługiwane. Istniejące skrzyżowania są zachowywane. Modyfikacja wysokości terenu obejmuje dopasowanie ASC pod wygenerowanymi partami wybranej trasy.
- Nowe drogi wymagają w bazowym TV4P przykładowej drogi wybranego typu; drogi wielosegmentowe wymagają także co najmniej jednego istniejącego segmentu odgałęzienia, używanego jako szablon rekordu. Pusty projekt należy najpierw przygotować w Terrain Builder.
- Dopasowanie segmentów jest heurystyczne. A* może znaleźć przebieg, którego dostępne modele nie odtworzą przy zadanych promieniach i tolerancji. Edytor zgłasza taki przypadek. Nie obiecuje znalezienia każdej możliwej drogi. Końcówka może odbiegać od punktu docelowego w granicach ustawionej tolerancji.
- Profile istniejących dróg przedstawiają ich części kolejno, bez łączenia rozdzielnych odgałęzień fikcyjnymi odcinkami.
- Wyszukiwanie ma budżet miliona odwiedzonych komórek, a dopasowanie 20000 segmentów. Długie trasy dziel punktami pośrednimi lub zwiększ krok automatu.
- Eksport zachowuje rekordy niewybranych dróg oraz zawartość poza blokiem dróg, aktualizując metadane rozmiaru. Sprawdza składnię, identyfikatory i ciągłość nowych segmentów. GUI sprawdza też nowe drogi względem bieżących ograniczeń terenu. Format TV4P nie ma pełnej specyfikacji; testy odczytu nie zastępują sprawdzenia w Terrain Builder.

## Weryfikacja na lokalnym projekcie

```powershell
$env:ROAD_TEST_TV4P = 'D:\Projekty\DayZRoadsMerge\output.tv4p'
cargo test optional_local_tv4p_roundtrip
```

Test bez zmian porównuje cały zapisany projekt bajt po bajcie z wejściem. Nie modyfikuje pliku wejściowego. Testy syntetyczne pokrywają zapis i odczyt nowych segmentów, obrót zakrętów, zachowanie pozostałych danych, raster i ASC.

Źródła wykorzystane przy projektowaniu: [eframe](https://github.com/emilk/egui/tree/main/crates/eframe), [strumieniowy odczyt PNG](https://docs.rs/png/0.18.1/png/struct.Reader.html). Odczyt MLOD i TV4P pochodzi z lokalnego DayZRoadsMerge; podziękowania również dla [WoozyMasta/tv4p-road-tool](https://github.com/WoozyMasta/tv4p-road-tool), wskazanego przez autora projektu bazowego.

## PNG i kolorystyka

Builder używa tej samej ciemnej kolorystyki co Merge: niebieskie drogi, żółte zaznaczenie. Pasek PNG na dole pozwala eksportować wszystkie drogi lub zaznaczoną, ustawić rozdzielczość, przezroczystość i pełny zasięg mapy. Przycisk 15360² ustawia rozdzielczość 15360 × 15360 px. Opcja Mapa korzysta z początku i wymiarów mapy w ustawieniach projektu; po jej wyłączeniu obraz dopasowuje się do dróg. Eksport obejmuje aktualną geometrię istniejących dróg i dopasowanych segmentów, uwzględniając przesunięcia, obroty i usunięcia. Szkice bez segmentów, satelita i poziomice nie trafiają do PNG. Obliczenia działają w tle także po zmianie zakładki.

W lewym panelu rozwiń **Kolory typów dróg** i kliknij próbkę obok nazwy typu, np. asf2. **Reset** przywraca domyślny kolor. Kolory zapisują się w `.dzroad` i są używane w PNG również przy eksporcie zaznaczonej drogi. Zaznaczenie w podglądzie pozostaje żółte. Ustawienia kolorów nie zmieniają pliku TV4P.
