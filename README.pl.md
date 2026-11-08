# Kanał filmowy dla VRChat

Serwer, który z linków do YouTube, pixeldrain, CDA i innych stron robi **jeden ciągły kanał telewizyjny HLS**.
Link kanału wklejasz raz do odtwarzacza wideo w świecie VRChat, a filmy z kolejki lecą po kolei.
Wszyscy w lobby widzą ten sam moment filmu.

- Okno odtwarzacza / panel właściciela (`/admin`): podgląd na żywo, pauza, przewijanie, „Odtwórz teraz”, kolejka i historia, statystyki dla nerdów, ustawienia.
- Publiczna strona zgłoszeń (`/`): widzowie dodają filmy pod swoim nickiem, z limitami i opcjonalnym zatwierdzaniem.
- Pasek informacyjny (opcja): pas wypalany w obrazie z tytułem, autorem zgłoszenia, następnym filmem, Twoimi wiadomościami, kodem QR strony zgłoszeń i paskiem postępu.
- Program mówi po angielsku, polsku, hiszpańsku, niemiecku, francusku i japońsku (beta).
- Gdy kolejka jest pusta, kanał nadaje planszę „Brak filmów w kolejce” i nie zrywa połączenia.
- Możesz puszczać też filmy z własnego dysku.

English version: [README.md](README.md).

> **Program jest do prywatnego oglądania ze znajomymi.** Za puszczane treści odpowiada osoba, która uruchamia serwer.
> Retransmisja cudzych utworów bez praw może naruszać prawo autorskie i regulaminy serwisów, z których pochodzą.

## Aplikacja na pulpit (Windows)

`VRCinema.exe` to cały serwer w jednym oknie: bez Node.js, bez instalatora, bez pliku `.env`.

- **Pobranie**: `VRCinema.exe` ze strony [Releases](https://github.com/wordoftiger-ux/vrcinema/releases) (około 25 MB). Połóż go gdziekolwiek, np. na pulpicie. Windows 10/11, 64-bitowy; potrzebny jest składnik Microsoft Edge WebView2 Runtime, który jest w Windows 11 i w większości instalacji Windows 10 (jeśli go brakuje, program podpowie, skąd go wziąć).
- **Pierwsze uruchomienie**: kreator pyta o język (na start ten z Windows), pobiera narzędzia (ffmpeg, yt-dlp, środowisko Deno potrzebne dla YouTube i cloudflared do linku publicznego, razem około 300 MB) do `%LOCALAPPDATA%\VRCinema`, sprawdza kartę graficzną i wybiera enkoder (NVIDIA, Intel, AMD albo procesor), uruchamia kanał i pokazuje jego link publiczny. Potem program od razu otwiera odtwarzacz.
- **SmartScreen**: plik nie jest podpisany certyfikatem, więc Windows może pokazać „System Windows chronił ten komputer”. Kliknij **Więcej informacji**, potem **Uruchom mimo to**.
- **Dodawanie filmów**: wklej link w pole nad kolejką (albo naciśnij Ctrl+V gdziekolwiek poza polem tekstowym) albo przeciągnij pliki wideo lub całe foldery na okno. Przeciągnięte pliki są odtwarzane z miejsca, w którym leżą na dysku.
- **Zasobnik**: zamknięcie okna zostawia kanał w zasobniku (za pierwszym razem pokaże się dymek). Menu zasobnika otwiera okno, kopiuje link kanału albo link strony zgłoszeń, pomija bieżący film i kończy program. **Zakończ** zatrzymuje kanał i wszystkie procesy ffmpeg.
- **Panel zdalny**: domyślnie wyłączony; panel jest dostępny tylko z tego komputera. Aby zarządzać kanałem z innego urządzenia (telefonu, laptopa), ustaw hasło w **Ustawienia** → **Panel zdalny** (zobacz [Panel zdalny i telefon](#panel-zdalny-i-telefon)).
- **Aktualizacje**: program sprawdza, czy jest nowszy `VRCinema.exe`, przy starcie i co 24 godziny, i proponuje instalację (plik jest weryfikowany sumą SHA-256, a stary zostaje, dopóki nowy się nie uruchomi). Wymaga to publicznych wydań w repozytorium.
- **Dane**: wszystko leży w `%LOCALAPPDATA%\VRCinema` (`config.json`, `tools\`, `data\`, `logs\`). Zmienna `VRCINEMA_HOME` wskazuje inny folder. Usunięcie folderu przywraca stan początkowy.
- **Rozmiar i pamięć** (zmierzone, wersja release): `VRCinema.exe` ma 25 MB. Podczas filmu przy otwartym oknie sam zajmuje około 45 MB; procesy WebView2 rysujące okno zajmują około 600 MB, gdy gra podgląd na żywo; cloudflared zajmuje około 40 MB, a ffmpeg to osobny proces (około 30 MB przy kopiowaniu, około 300 MB przy konwersji na karcie graficznej).
- **Bez okna** (serwer, Docker): zobacz [VPS (Linux, Docker)](#vps-linux-docker), [Wersja w Rust](#wersja-w-rust-eksperymentalna) i [Linux (serwer bez okna)](#linux-serwer-bez-okna) niżej; serwer `vrcinema` bez okna ma ten sam panel i tę samą stronę zgłoszeń.

### Odtwarzacz

Okno wygląda i działa jak odtwarzacz multimediów. Nie ma ramki Windows: pasek 32 px u góry to pasek tytułu (przeciągnij go, żeby przesunąć okno, kliknij dwukrotnie, żeby zmaksymalizować; przyciski minimalizacji, maksymalizacji i zamknięcia są po prawej; przyciąganie do krawędzi Windows działa). Zamknięcie chowa okno do zasobnika. Okno pamięta rozmiar, położenie i maksymalizację.

- **Pasek boczny po lewej** z czterema widokami:
  - **Kanał**: dwa ekrany obok siebie, **Podgląd** (najświeższy materiał, opis niżej) i **Na antenie** (strumień tak, jak dostają go widzowie, „Na antenie · ok. 12 s za serwerem”), kolejka **Następne** (przeciąganie zmienia kolejność; menu pozycji ma **Odtwórz teraz**, **Na górę**, **Kopiuj link** i **Usuń z kolejki**), pole linku, **Do zatwierdzenia** przy włączonym zatwierdzaniu i miejsce na upuszczanie plików.
  - **Historia**: co już leciało (odtworzony, pominięty, błąd) z **Odtwórz teraz** / **Spróbuj ponownie** i **Do kolejki**.
  - **Udostępnij**: link kanału dla VRChat, strona zgłoszeń z kodem QR, stan internetu i stały link.
  - **Ustawienia**: Ogólne (język, autostart, sieć domowa, port), Internet, Kanał (linki od widzów, limity, zatwierdzanie, język plansz, pasek informacyjny, nicki widzów, konwersja), Narzędzia, Panel zdalny, Zaawansowane, Dziennik, O programie.
- **Pasek odtwarzania na dole**: tytuł i kto dodał film; −10 s, ⏮ (od początku), pauza/wznów, ⏭ (następny film), +10 s; oś czasu do kliknięcia albo przeciągnięcia (pokazuje czas pod kursorem); głośność podglądu; stan internetu; **Kopiuj link** (link kanału do odtwarzacza VRChat).
- **Skróty klawiszowe** (gdy fokus nie jest w polu tekstowym; **?** pokazuje listę):

  | Klawisz | Działanie |
  |---|---|
  | Spacja | Pauza / wznów |
  | ← / → | 10 s wstecz / naprzód |
  | Shift + ← / → | 60 s wstecz / naprzód |
  | Home | Od początku tego filmu |
  | N | Następny film |
  | I | Statystyki dla nerdów |
  | S | Zsynchronizuj Podgląd z anteną |
  | Ctrl + V | Dodaj link ze schowka do kolejki |
  | ? | Skróty klawiszowe |

- **Menu kontekstowe**: prawy przycisk na pozycji kolejki albo na ekranie „Na antenie” (jego menu ma **Statystyki dla nerdów** i **Kopiuj link kanału**). Z klawiatury otwiera je Shift+F10 albo klawisz menu; Alt+↑/↓ przesuwa pozycję kolejki z fokusem.

### Sterowanie i opóźnienie

- **Pauza** pokazuje na kanale „Pauza — wracamy za chwilę” i zapamiętuje pozycję; **Wznów** rusza od tego samego miejsca po kilku sekundach planszy buforowania. Pauza, przewijanie i ⏮ są wyłączone dla transmisji na żywo (⏭ działa).
- **Przewijanie** (±10 s, ±60 s, oś czasu, ⏮) uruchamia ten sam film od nowej pozycji. Kilka szybkich kliknięć łączy się w jeden skok (program czeka 0,7 s po ostatnim), więc osiem kliknięć +10 s to jeden skok o 80 s. Przewinięcie w pauzie tylko przesuwa zapamiętaną pozycję. Skok na ostatnią sekundę puszcza następny film.
- **Dokładność**: film kopiowany (bez konwersji) skacze do klatki kluczowej w celu albo przed nim, najwyżej o odstęp klatek kluczowych źródła wcześniej, najwyżej ok. 1,25 s (tylko takie źródła są kopiowane; czas pokazuje „≈”); film konwertowany (np. przy włączonym pasku informacyjnym) skacze dokładnie.
- **Odtwórz teraz** (kolejka albo Historia) od razu puszcza film; bieżący kończy się jako pominięty. Z Historii film jest dodawany ponownie jako nowa pozycja (ten sam link i nick).
- **Opóźnienie**: kanał jest cięty na 1-sekundowe fragmenty; playlista podaje docelową długość 4 s, więc odtwarzacze startują ok. 12 s za krawędzią na żywo (jak wcześniej przy 4-sekundowych fragmentach), więc **widzowie widzą każdą zmianę (pauzę, skok, następny film) ok. 10–15 sekund po kliknięciu**. Ekran „Na antenie” ma to samo opóźnienie; po skoku dymek mówi „Widzowie zobaczą to za ok. N s”. Ekran „Podgląd” pokazuje zmianę kilka sekund po kliknięciu (następna sekcja).
- **Pauza ręczna przetrwa restart serwera** (nowy port, dostęp z sieci domowej, naprawa narzędzi), **ale nie ponowne uruchomienie programu**: po zakończeniu i ponownym uruchomieniu VRCinema kanał gra.

### Podgląd i Na antenie

Widok **Kanał** ma dwa ekrany, jak program do transmisji:

- **Podgląd** (po lewej) gra najświeższe fragmenty, jakie serwer już ma, w tempie rzeczywistym: po przewinięciu, **Odtwórz teraz**, ⏮ albo ⏭ pokazuje nowy obraz, gdy tylko ffmpeg wytnie pierwszy fragment (ok. 1–2 s dla pliku; link, który musi rozpoznać yt-dlp, trwa dłużej), a nie po 10–15 s opóźnienia widzów. Podpis mówi, o ile wyprzedza antenę („Podgląd · ok. 12 s przed anteną”). Startuje wyciszony (głośnik obok podpisu włącza jego dźwięk; głośność na pasku odtwarzania dotyczy ekranu „Na antenie”). Gdy kanał jest wstrzymany, stoi na obrazie, od którego film ruszy po wznowieniu.
- **Sterowanie liczy od Podglądu**: pauza (przycisk, spacja) od razu zatrzymuje Podgląd na klatce, którą widzisz, i kanał wstrzymuje się w tym miejscu: „Na antenie” gra dalej do końca fragmentu z tą klatką (pasek odtwarzania pisze „Wstrzymywanie”), potem staje; wznowienie rusza od końca tego fragmentu (najwyżej jeden fragment, 1 s, za zatrzymaną klatką; Podgląd startuje stamtąd), więc widzowie niczego nie tracą ani nie widzą dwa razy. Jeśli serwer wysłał już tę klatkę dalej, pauza jest natychmiastowa. Przewinięcie w pauzie (także gdy pauza jeszcze czeka) tylko przesuwa miejsce wznowienia; „Na antenie” i tak gra najpierw do miejsca pauzy. Jeśli film skończy się przed miejscem pauzy, kanał staje na jego końcu, a następny film czeka na **Wznów**. ±10 s / ±60 s (przyciski i strzałki) skaczą od pozycji Podglądu, a pasek odtwarzania pokazuje czas Podglądu; tuż po przewinięciu pokazuje cel (Podgląd może jeszcze przez kilka sekund pokazywać stary obraz), więc kolejne ±10 s liczy się od celu. Bez Podglądu (panel zdalny, telefon, nic na nim nie ma) liczy się pozycja serwera, jak dotąd.
- **Na antenie** (po prawej) to strumień kanału dokładnie tak, jak gra go VRChat, z podpisem opóźnienia, statystykami dla nerdów i menu pod prawym przyciskiem.
- **Zsynchronizuj z anteną** (przycisk pod Podglądem, klawisz **S**) ustawia Podgląd na to, co widzowie widzą teraz. Tuż po pominięciu filmu tego, co jest na antenie, może już nie być; wtedy pojawia się komunikat. Przy transmisji na żywo przycisk jest wyłączony.
- Podgląd nie uruchamia dodatkowego ffmpeg: gra fragmenty filmu, który właśnie jest cięty. Każdy fragment, który pokazał na liście, zostaje do odczytu przez 90 s po wyemitowaniu (łącznie najwyżej 256 MiB, w `clips\_monitor` folderu danych; usuwane, gdy kanał staje; nieruchomy obraz pauzy zostaje tak długo, jak trwa pauza), więc pliki filmu są sprzątane dokładnie jak wcześniej. Serwer czyta z wyprzedzeniem dla Podglądu tylko wtedy, gdy okno go pokazuje.
- Podgląd jest tylko w oknie programu. W panelu zdalnym jest wyszarzony („Dostępne w programie na komputerze”); na telefonie widać tylko „Na antenie”.

### Statystyki dla nerdów

Naciśnij **I** albo prawy przycisk na podglądzie → **Statystyki dla nerdów**: nakładka jak na YouTube, odświeżana co sekundę, gdy jest otwarta. Pokazuje film na antenie (źródło, rozdzielczość, kopiowanie albo konwersja, koder, gotowe fragmenty), kanał HLS (sekwencja, nieciągłości, czy znaczniki czasu są ciągłe), następny film, ffmpeg (prędkość, CPU, pamięć), widzów (odtwarzacze, które pobrały fragment w ostatnich 30 s, pobrania na minutę, wysyłanie z wykresem 60 s), tunel (stan, czas w sieci, ponowne połączenia, lokalizacje Cloudflare) i podgląd w oknie (opóźnienie, bufor, porzucone klatki). **Kopiuj** kopiuje całość jako tekst. Adresy widzów są tylko liczone, nigdy pokazywane.

### Języki

- Trzy osobne wybory: **program** (okno, panel zdalny, zasobnik, dziennik: **Ustawienia** → **Ogólne** → **Język** i pierwszy krok kreatora; na start język Windows, jeśli jest jednym z sześciu, inaczej angielski), **plansze i pasek informacyjny** (**Ustawienia** → **Kanał** → **Język plansz**, domyślnie angielski — to język, który czytają widzowie w VRChat) i **strona zgłoszeń** (każdy widz wybiera go przełącznikiem u góry; na start język przeglądarki, wybór zapamiętuje przeglądarka).
- Języki: English, Polski, Español, Deutsch, Français i 日本語 (beta). Komunikaty błędów serwera przychodzą w języku widza albo panelu.
- **Serwer bez okna** (`vrcinema` z `.env`): angielski, chyba że w `.env` jest `LANGUAGE=pl` (albo `es`, `de`, `fr`, `ja`); jego panel nie ma przełącznika języka.
- **Japoński to beta**: teksty przetłumaczono bez rodzimego użytkownika. Wszystkie japońskie teksty z angielskim wzorcem i kontekstem są w [docs/i18n/ja-review.md](docs/i18n/ja-review.md) do przeglądu przez rodzimego użytkownika. Plansze i pasek używają czcionki Windows Yu Gothic (zapasowo Meiryo); bez obu plansze zostają po angielsku (z wpisem w dzienniku).
- **Pomoc w tłumaczeniu**: słowniki to `public/locales/<język>.json` (wzorcem jest angielski `en.json`). `public/locales/context.json` mówi, gdzie występuje każdy tekst i jaki ma limit długości, a [docs/i18n/glossary.md](docs/i18n/glossary.md) ustala pojęcia. Zachowaj `{pola}` i kategorie liczby mnogiej; `cargo test` w `engine/` sprawdza w każdym języku brakujące klucze, pola, liczbę mnogą i długości.

### Pasek informacyjny

Opcjonalny pas wypalany w obrazie strumienia, więc widzi go każdy w VRChat: „▶ Teraz: <tytuł> · dodane przez <nick | Gospodarza> · następnie: <tytuł> · dodaj film: <adres>” i Twoje wiadomości, kod QR strony zgłoszeń w prawym dolnym rogu z podpisem „Dodaj film” i cienki pasek postępu przy dolnej krawędzi.

- **Ustawienia** → **Kanał** → **Pasek informacyjny**: wł./wył.; tryb **Co jakiś czas** (co 1, 2, 5 albo 10 minut; za każdym razem informacja o filmie, a po niej kolejna z Twoich wiadomości) albo **Ciągły** (informacja i wszystkie wiadomości w pętli); prędkość przewijania (panel pokazuje, jak długo tekst jest widoczny); pokazywanie następnego filmu; pokazywanie adresu; kod QR **Wyłączony**, **Razem z paskiem** albo **Zawsze**; pasek postępu; do 10 wiadomości po 200 znaków (nie są tłumaczone).
- **Sprzęt**: przy włączonym pasku każdy film jest konwertowany (pasek jest rysowany w obrazie), co jest lekkie na karcie graficznej NVIDIA, Intel albo AMD i ciężkie na procesorze. Domyślnie włącza się tylko wtedy, gdy kreator znalazł koder na karcie graficznej. Jeśli komputer nie nadąża (konwersja wolniejsza niż czas rzeczywisty przez 30 s), mówią o tym dziennik i panel: wyłącz pasek albo użyj karty graficznej (**Ustawienia** → **Narzędzia**).
- **Kiedy widać zmiany**: tekst (Twoje wiadomości, „następnie”, adres, język plansz, zmiany kolejki) zmienia się na strumieniu w ciągu ok. **20–60 sekund** (film jest przygotowywany z wyprzedzeniem; ok. 20 s dla filmu na antenie, do ok. minuty dla filmu przygotowanego wcześniej). Włączenie albo wyłączenie paska, tryb, odstęp, prędkość, kod QR i pasek postępu uruchamiają film na antenie od nowa od bieżącego miejsca (kilka sekund planszy buforowania); tak samo nowy adres publiczny, gdy pokazywany jest kod QR.
- Tekst jest biały z cieniem na ciemnym pasie; pasek postępu jest bursztynowy. Transmisje na żywo nie mają paska postępu.

### Chatbox VRChat (OSC)

Program może pokazywać film na antenie w **Twoim własnym chatboxie VRChat** (dymek nad awatarem), np. „▶ Never Gonna Give You Up — dodane przez Kasia”.

- **Ustawienia** → **Kanał** → **Chatbox VRChat**: włącz/wyłącz (domyślnie wyłączone), adres i port (domyślnie `127.0.0.1` i `9000`, wejście OSC VRChat), tekst z polami `{title}`, `{nick}` (nick widza, pusty dla Twoich filmów) i `{added}` („dodane przez …” w języku plansz; domyślnie `▶ {title} — {added}`) oraz **Wysyłaj ponownie** (tylko przy zmianie filmu albo co 10 s – 5 min, bo VRChat po chwili chowa chatbox).
- **Dociera do VRChat tylko wtedy, gdy program działa na tym samym komputerze co VRChat**, albo gdy adres wskazuje komputer z VRChat (jego adres w sieci domowej) i ten komputer przyjmuje UDP na tym porcie. OSC musi być włączone w VRChat: **Action Menu** → **Options** → **OSC** → **Enabled**. Na VPS-ie czy Raspberry Pi domyślny adres nigdzie nie trafia.
- Co jest wysyłane: jedna wiadomość OSC `/chatbox/input` z tekstem, „wyślij od razu” (żeby nie otwierała się klawiatura VRChat) i „bez dźwięku powiadomienia”; przy zmianie filmu i przy powtórce, nigdy częściej niż raz na 1,5 s (limit VRChat). Tytuły i nicki pochodzą od widzów: znaki nowej linii i sterujące są usuwane, a tekst przycinany do 144 znaków (limit VRChat).
### Nicki widzów

- Widzowie mogą wpisać nick (do 24 znaków) na stronie zgłoszeń. Pierwsze zgłoszenie z nickiem tworzy w tej przeglądarce losowy klucz urządzenia; serwer zapisuje tylko jego skrót SHA-256. Nick należy potem do tego urządzenia przez 30 dni od ostatniego użycia: ten sam nick z innej przeglądarki albo urządzenia jest odrzucany („Ten nick jest już zajęty przez kogoś innego”). Podobne nicki liczą się jako ten sam: wielkość liter, znaki diakrytyczne, spacje, kropki, myślniki, niewidoczne znaki i podobne litery cyrylicy albo greki są pomijane (Kasia, KASIA, kásia, K a s i a, Ka5ia to jeden nick). „Gospodarz”, „Host” i ich tłumaczenia są zastrzeżone.
- Przy nicku panel i strona zgłoszeń pokazują ikonkę (kolor i kształt) wyliczaną z klucza urządzenia. To **pomoc wzrokowa** do wykrywania podszywania się, nie dowód tożsamości. Twoje filmy mają znaczek **Gospodarz**.
- **Ustawienia** → **Kanał** → **Nicki widzów** pokazuje nicki z ikonkami; **Zwolnij** zwalnia nick, **Zablokuj urządzenie** odrzuca zgłoszenia z tego urządzenia (a **Odblokuj** zdejmuje blokadę).
- Ograniczenia: nick wymaga klucza urządzenia, więc działa tylko w przeglądarce, która zachowuje dane strony; widz w oknie prywatnym albo po wyczyszczeniu danych strony dostaje nowy klucz, więc stary nick pokazuje się jako zajęty; może wybrać inny nick albo wysłać bez nicku. Zablokowany widz może dalej wysyłać zgłoszenia **bez nicku** z nowego profilu przeglądarki (anonimowo); limity zgłoszeń na adres nadal działają, a zatwierdzanie (**Ustawienia** → **Kanał** → zatwierdzanie) zatrzyma wszystko, czego nie chcesz.

### Panel zdalny i telefon

- Panel zdalny to ten sam interfejs co okno, pod `<adres publiczny>/admin` (albo `http://<ten komputer>:<port>/admin` przy dostępie z sieci domowej), po zalogowaniu hasłem z **Ustawienia** → **Panel zdalny**. Może wszystko to, co okno, **poza pięcioma rzeczami, które zostają w oknie**: port i dostęp z sieci domowej, hasło i wyłączenie panelu zdalnego, **Ufaj nagłówkom Cloudflare** (od tego zależy, od kogo przychodzi żądanie), logowanie do Cloudflare (a więc ustawienie i usunięcie stałego linku) oraz wybór pliku cookies (okno wyboru pliku otwiera się na komputerze; panel zdalny może plik odłączyć i widzi tylko jego nazwę, bez folderu). Upuszczanie plików i kreator pierwszego uruchomienia też są tylko w oknie. Te miejsca pokazują „Dostępne w programie na komputerze”.
- **Wylogowanie**: **Ustawienia** → **Ogólne** → **Wyloguj** (na telefonie: ikona w prawym górnym rogu). Zmiana hasła albo wyłączenie panelu zdalnego wylogowuje wszystkie zdalne urządzenia.
- **Przełącznik tunelu**: gdy panel otwarto przez tunel, wyłączenie tunelu (albo ponowne połączenie, albo zmiana między szybkim a stałym linkiem) najpierw pyta, bo kończy Twoje własne połączenie („Stracisz dostęp do tego panelu”).
- **Układ na telefon**: poniżej 700 px szerokości pasek boczny staje się zakładkami na dole, a pasek odtwarzania jest pod podglądem z większymi przyciskami. Klucz API pixeldrain można podmienić, ale nigdy nie jest pokazywany z powrotem.

### Link publiczny (wbudowany tunel)

Program sam udostępnia kanał w internecie: bez konta, bez terminala i bez wpisywania adresu.

- **Szybki tunel, domyślnie włączony**: przy starcie program uruchamia własną kopię [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) i dostaje adres w stylu `https://abc-def.trycloudflare.com`. Pasek odtwarzania pokazuje **W sieci**, a **Kopiuj link** kopiuje link kanału (`<adres>/live/main.m3u8`); link kopiuje też menu zasobnika.
- **Adres zmienia się przy każdym uruchomieniu programu** (i gdy tunel musi połączyć się od nowa). Podaj znajomym nowy link albo ustaw stały link (niżej).
- **Zastępuje własny cloudflared.** Jeśli przy poprzedniej wersji uruchamiałeś sam `cloudflared tunnel --url …`, nie musisz już tego robić. Przy włączonym wbudowanym tunelu ręcznie wpisany adres publiczny (**Ustawienia** → **Zaawansowane**) i zaufanie do Cloudflare nie są używane; działają tylko po wyłączeniu tunelu w **Ustawieniach** → **Internet**. Konfiguracja z poprzedniej wersji też dostaje włączony tunel, a brakujący cloudflared program pobiera sam w tle (gdy się nie uda, np. bez internetu, próbuje ponownie co 5 minut).
- **Kod QR na planszy oczekiwania**: gdy kolejka jest pusta, kanał pokazuje „Brak filmów w kolejce”, kod QR strony zgłoszeń i pod nim jej adres. Znajomi w VRChat skanują go telefonem i dodają film. Nowy adres podmienia kod na żywo, bez restartu. Bez adresu plansza mówi „Łączenie z internetem…”. (Plansze są w języku plansz, domyślnie po angielsku: „No videos in the queue”.)
- **Wyłączenie**: **Ustawienia** → **Internet** → „Udostępniaj kanał w internecie”. Bez tunelu kanał jest dostępny tylko z tego komputera (albo pod adresem wpisanym ręcznie).
- **VRChat i niezaufane adresy**: każdy widz musi włączyć w VRChat opcję **Allow Untrusted URLs**. Według forum zgłoszeń VRChat od wersji 1546 ta opcja nie otwiera już każdego adresu w instancjach **Public** i **Group Public**: grają tam tylko domeny dopuszczone przez twórcę świata. Link `trycloudflare.com` może więc działać tylko w instancjach Friends+, Friends, Invite albo Group, a stały link na własnej domenie tego nie zmienia, chyba że świat dopuszcza Twoją domenę. Sami tego nie sprawdziliśmy: wypróbuj w rodzaju instancji, którego używasz (zobacz listę testów niżej).

### Stały link (własna domena)

Link, który się nie zmienia, wymaga darmowego konta [Cloudflare](https://dash.cloudflare.com/sign-up) i własnej domeny:

1. Kup domenę u dowolnego rejestratora (wystarczy tania) i [dodaj ją do konta Cloudflare](https://developers.cloudflare.com/fundamentals/setup/manage-domains/add-site/): Cloudflare poda dwa serwery nazw, które ustawiasz u rejestratora. Poczekaj, aż Cloudflare pokaże domenę jako **Active**.
2. W VRCinema otwórz **Ustawienia** → **Internet** → **Ustaw stały link**. Program raz otworzy przeglądarkę: zaloguj się do Cloudflare i zatwierdź domenę. Program czeka na logowanie 5 minut; **Anuluj** je przerywa, a po przerwaniu albo upływie czasu **Spróbuj ponownie** zaczyna od nowa. Na koncie bez domeny logowanie nigdy się nie kończy (najpierw dodaj domenę, krok 1).
3. Wpisz pełną nazwę hosta dla kanału, np. `kanal.example.com`. Program tworzy tunel o nazwie `vrcinema` i rekord DNS dla tej nazwy. Jeśli nazwa ma już rekord, zapyta, czy go nadpisać, czy wybrać inną nazwę.

Od tej chwili link kanału to `https://kanal.example.com/live/main.m3u8`, także po restarcie i zmianie portu. **Wróć do szybkiego tunelu** przełącza z powrotem na szybki tunel (stały link zostaje ustawiony); **Usuń stały link** usuwa tunel (jeśli rekord DNS nadal istnieje, usuń go w panelu Cloudflare). Certyfikat Cloudflare i poświadczenia tunelu leżą w `%LOCALAPPDATA%\VRCinema\tunnel\` i nigdy nie trafiają do dziennika ani na ekran.

### Automatyczna pauza

Widzowie nie tracą kawałka filmu, gdy zerwie się połączenie:

- **Utrata internetu** (domyślnie włączona): gdy tunel nie działa przez 10 sekund, film się zatrzymuje, a kanał pokazuje „Pauza — wracamy za chwilę”. Po powrocie tunelu film wznawia się od tego samego miejsca. Krótsze przerwy niczego nie wstrzymują.
- **Nikt nie ogląda** (domyślnie wyłączona): po 30 sekundach bez widzów film się zatrzymuje i wznawia, gdy tylko ktoś zacznie oglądać (podgląd w oknie liczy się jako widz, gdy okno jest widoczne; zminimalizowane okno albo schowane do zasobnika zatrzymuje podgląd).
- Transmisje na żywo nigdy nie są wstrzymywane. Obie opcje są w **Ustawieniach** → **Internet** → „Wstrzymywanie kanału”. Wznowiony film rusza po kilku sekundach planszy buforowania; tak ma być.

### Nadawanie z OBS

OBS Studio może nadawać prosto do kanału (RTMP). W aplikacji otwórz **Ustawienia** → **OBS** i skopiuj dwie wartości do OBS → Settings → Stream: Service „Custom…”, **Server** `rtmp://127.0.0.1:1935/live` i pokazany tam **klucz strumienia** (zachowaj go dla siebie; „Nowy klucz” od razu go wymienia).

- **OBS ma priorytet nad kolejką** (domyślnie włączone): gdy OBS zacznie nadawać, film na antenie zatrzymuje się dokładnie w miejscu, a OBS wchodzi na antenę w ciągu kilku sekund; gdy zakończysz transmisję, film rusza od tego samego miejsca. Wyłączone: OBS czeka na koniec filmu na antenie.
- Krótkie zerwanie pokazuje „Zaraz wracamy” przez maksymalnie 10 sekund; jeśli OBS wróci w tym czasie, nadaje dalej, w przeciwnym razie wraca kolejka.
- Obraz z OBS zawsze przechodzi przez konwersję kanału (klatka kluczowa w każdym fragmencie, dźwięk 48 kHz stereo, najwyżej 1080p30), więc OBS może wysyłać cokolwiek. Pasek informacyjny i kod QR są rysowane jak na każdej transmisji na żywo. Podgląd pokazuje OBS na żywo; przewijanie i pauza są wyłączone, dopóki OBS jest na antenie (zatrzymaj transmisję w OBS, aby wrócić do kolejki).
- **Zezwól na OBS z innych urządzeń w sieci** (domyślnie wyłączone): połączyć może się tylko OBS na tym komputerze. Po włączeniu OBS na innym komputerze w sieci domowej użyje adresu z „Serwer dla innych urządzeń”. Windows zapyta wtedy raz, czy VRCinema może przyjmować połączenia przez zaporę: zezwól tylko dla **sieci prywatnych**. Port OBS (1935) to osobny port, którego tunel nigdy nie przekazuje, więc z internetu przez publiczny link kanału nie da się do niego dostać. Pięć złych kluczy z jednego adresu w ciągu minuty blokuje ten adres na minutę.
- Serwer bez okna (`.env`) przyjmuje OBS tylko z `OBS_STREAM_KEY` (co najmniej 16 znaków); `OBS_PORT`, `OBS_LAN=1` i `OBS_PRIORITY=0` zmieniają resztę.

### Przechwytywanie z tego komputera (okno, ekran, kamera, mikrofon)

W oknie programu przycisk **Przechwyć** pod polem linku dodaje do kolejki okno, wszystkie ekrany, jeden ekran, fragment pulpitu, kamerę albo sam mikrofon tego komputera jako pozycję na żywo (wchodzi na antenę, gdy przyjdzie jej kolej; nie ma priorytetu jak OBS). Wybierz urządzenie dźwięku, liczbę klatek (15/25/30), rozdzielczość (480p–1080p, nigdy powyżej Ustawienia → Zaawansowane), tytuł dla widzów (tytuły okien i nazwy urządzeń nigdy nie trafiają do widzów ani do dziennika) i sprawdź jedną klatkę przyciskiem **Podgląd**. Obraz jest wpasowany w kadr 16:9 i konwertowany jak transmisja na żywo (klatka kluczowa w każdym fragmencie, 48 kHz stereo, kodery GPU, pasek informacyjny i QR); Podgląd w widoku Kanał pokazuje go na żywo.

- Podczas przechwytywania pasek tytułu pokazuje czerwone **Przechwytywanie**; **Zatrzymaj** kończy od razu wszystkie przechwytywania i usuwa czekające.
- Przechwytywać może tylko samo okno: panel zdalny, tunel i widzowie nie mogą wyświetlić urządzeń ani dodać przechwytywania (403), nie da się go też dodać jako linku. Przechwytywanie żyje tylko do Zatrzymaj albo końca programu: po restarcie lub odtworzeniu z historii nie rusza ponownie (dodaj je od nowa).
- Windows: okna po tytule (gdigrab; przechwytywanie kończy się, gdy tytuł się zmieni lub okno zostanie zamknięte), ekrany po prostokącie albo z **szybkim przechwytywaniem** (Desktop Duplication, `ddagrab`; przy kilku kartach graficznych numery ekranów mogą się różnić), kamery i mikrofony przez DirectShow. Dźwięk odtwarzany na komputerze: włącz „Miks stereo” w ustawieniach dźwięku albo zainstaluj wirtualny kabel audio.
- Linux (w miarę możliwości): ekran X lub jego fragment (`x11grab`, `$DISPLAY`), kamery (`/dev/video*`), źródła PulseAudio/PipeWire (`.monitor` = to, co gra) albo ALSA. Pojedyncze okna nie są dostępne; pod Waylandem widać tylko treść XWayland. macOS: niedostępne.

### Lista testów ręcznych dla właściciela

Tego nie sprawdzają testy automatyczne, potrzebna jest osoba przy ekranie:

- [ ] Przechwytywanie: okno (i koniec przechwytywania po jego zamknięciu), jeden ekran z szybkim przechwytywaniem i bez, kamera z mikrofonem; czerwone Przechwytywanie pojawia się, gdy przechwytywanie jest na antenie, a Zatrzymaj kończy je w ciągu sekundy; widzowie widzą wybrany tytuł; panel zdalny nie ma przycisku Przechwyć.

- [ ] Czyste konto Windows (albo komputer, na którym programu nigdy nie było): pierwsze uruchomienie z pustego folderu, kreator (najpierw krok języka), pierwszy film leci.
- [ ] Okno bez ramki, główne i kreatora: przeciąganie za pasek tytułu, dwuklik maksymalizuje i przywraca, przyciski minimalizuj / maksymalizuj / zamknij (zamknięcie chowa do zasobnika), przyciąganie do krawędzi Windows (przeciągnięcie do krawędzi ekranu, Win+←/→/↑), zmiana rozmiaru za krawędzie; rozmiar i położenie wracają po restarcie. Sprawdź to w buildzie release (testy tylko symulują okno): nic tu nie może zawieść po cichu; odmowa pokazuje „Pasek tytułu nie mógł sterować oknem: …”.
- [ ] Drugi monitor z innym skalowaniem (np. 100 % i 150 %): przesuń tam okno, zakończ program, uruchom ponownie; otwiera się na tym monitorze we właściwym rozmiarze; zmaksymalizowane też.
- [ ] Pierwsze uruchomienie na małym albo gęstym ekranie (np. laptop 1366×768 albo 4K przy 200 %): okno mieści się na ekranie (minimum 960×600) i nic nie jest ucięte.
- [ ] Minimalizacja na ok. 60 s i przywrócenie: podgląd wraca na żywo (nie 60 s z tyłu), a podpis z opóźnieniem się uspokaja; zminimalizowane albo schowane do zasobnika okno nie pobiera strumienia (w statystykach dla nerdów spada liczba widzów); okno tylko zasłonięte innym oknem gra dalej.
- [ ] Podgląd w buildzie release: puść film, naciśnij +60 s, ⏮, **Odtwórz teraz** na innym filmie i ⏭; lewy ekran zmienia się w ciągu kilku sekund, prawy ok. 12 s później; **Zsynchronizuj z anteną** (i S) sprawia, że oba pokazują to samo; pauza pokazuje nieruchomy obraz, wznowienie gra dalej; minimalizacja na minutę i przywrócenie: Podgląd znowu gra. Okno w minimalnym rozmiarze (960×600) nadal pokazuje oba ekrany.
- [ ] Panel zdalny na prawdziwym telefonie: logowanie pod `<adres publiczny>/admin`, zakładki na dole, pasek odtwarzania pod podglądem, pauza i przewijanie działają, elementy tylko-okna mówią „Dostępne w programie na komputerze”, wylogowanie działa; złe hasło jest odrzucane; wyłączenie panelu albo zmiana hasła wylogowuje telefon.
- [ ] `/desktop-login`: otwórz `http://127.0.0.1:<port>/desktop-login?key=cokolwiek` w zwykłej przeglądarce: kończy się na `/admin` z ekranem logowania, nigdy w panelu (okno używa własnego jednorazowego klucza).
- [ ] Pasek informacyjny w VRChat: przy włączonym pasku obejrzyj film w świecie VRChat: pas jest czytelny na ekranie świata z normalnej odległości, kod QR da się zeskanować telefonem z VRChat i otwiera stronę zgłoszeń, pasek postępu się przesuwa.
- [ ] Allow Untrusted URLs: puść link kanału w instancji **Public** albo **Group Public** oraz w instancji Friends albo Invite (z włączonym Allow Untrusted URLs). Zanotuj, gdzie działa (zobacz uwagę o VRChat wyżej).
- [ ] Przegląd japońskiego: rodzimy użytkownik czyta [docs/i18n/ja-review.md](docs/i18n/ja-review.md) (i program z wybranym 日本語) i przysyła poprawki.
- [ ] Przeciąganie od początku do końca: jeden plik, kilka plików, folder, plik niebędący filmem (pomijany); strona pokazuje wynik, film leci.
- [ ] Podążanie za zmianą portu: zmień port w **Ustawienia** → **Ogólne**; okno przeładowuje się pod nowym adresem.
- [ ] Wybór pliku cookies: wskaż plik w **Ustawienia** → **Zaawansowane** i dodaj link, który go wymaga.
- [ ] Autostart włącz/wyłącz: z Windows program startuje w zasobniku bez okna; wyłączenie usuwa wpis.
- [ ] Restart po aktualizacji: zainstaluj nowsze wydanie podstawione lokalnie; program uruchamia się ponownie, a stary plik znika.
- [ ] Kliknięcia w linki: zewnętrzne linki w panelu otwierają się w domyślnej przeglądarce, okno zostaje przy programie.
- [ ] Pozycje menu zasobnika: otwórz okno, kopiuj link kanału i link strony zgłoszeń, pomiń, zakończ; dwuklik w ikonę.
- [ ] Dymek przy zamknięciu do zasobnika: zamknięcie okna chowa je i pokazuje komunikat raz.
- [ ] Drugie uruchomienie: przy działającym programie (okno schowane do zasobnika albo zminimalizowane) uruchom `VRCinema.exe` jeszcze raz; istniejące okno wraca na wierzch i nie startuje druga kopia (także po **Zakończ**: nowe uruchomienie startuje normalnie w ciągu kilku sekund).
- [ ] Naprawa: przy uszkodzonych narzędziach (np. zamień `tools\ffmpeg.exe` w folderze danych na pusty plik i uruchom program ponownie) panel mówi, że kanał nie wystartował, i proponuje ponowne uruchomienie kanału oraz naprawę narzędzi (**Ustawienia** → **Narzędzia**); naprawa przywraca kanał.
- [ ] Stały link od początku do końca z prawdziwą domeną: **Ustaw stały link** raz otwiera przeglądarkę do logowania w Cloudflare; potem pojawia się krok z nazwą hosta; kanał działa pod `https://<host>/live/main.m3u8` i zachowuje ten link po restarcie programu i zmianie portu. Zakończenie VRCinema (zasobnik → **Zakończ**) w trakcie lub po logowaniu nie zamyka przeglądarki. Nazwa hosta, która ma już rekord DNS, pokazuje pytanie o nadpisanie, a nadpisanie działa. **Wróć do szybkiego tunelu** i **Usuń stały link** działają (tunel znika z panelu Cloudflare).
- [ ] Kod QR z VRChat: przy pustej kolejce zeskanuj telefonem kod QR z planszy oczekiwania w świecie VRChat; otwiera się strona zgłoszeń, a dodany tam film leci.
- [ ] Automatyczna pauza: w trakcie filmu odłącz kabel sieciowy (albo wyłącz Wi-Fi) na co najmniej 15 s; stan pokazuje „Brak internetu”, a film się zatrzymuje; po podłączeniu tunel wraca (z tym samym adresem, jeśli cloudflared działał dalej, z nowym, jeśli musiał uruchomić się od nowa), a film leci dalej od tego samego miejsca.
- [ ] Aktualizacja z poprzedniej wersji: uruchom nowy `VRCinema.exe` na folderze danych poprzedniej wersji (jej `config.json`); tunel jest włączony; program sam pobiera cloudflared w tle; panel pokazuje pobieranie, nigdy „kanał nie nadaje”, a potem link publiczny; film może się zatrzymać, jeśli pobieranie trwa dłużej niż 10 s. Linki od widzów zachowują dotychczasową listę dozwolonych serwisów (tryb „Tylko dozwolone serwisy”).

## Wymagania

- Windows 10/11, Linux albo macOS.
- [Node.js](https://nodejs.org) w wersji **22.18 lub nowszej** (zalecana 24).
- Do udostępnienia w internecie: [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) albo VPS.

ffmpeg i yt-dlp instalują się automatycznie do folderu `bin`.

> Serwer Node.js to poprzednia implementacja. Nowy interfejs z etapu 4 serwują tylko aplikacja na pulpit i serwer Rust (`vrcinema`, niżej); starej strony panelu serwera Node.js nie ma już w repozytorium, więc jako serwer bez okna używaj wersji Rust.

## Pierwsze uruchomienie (Windows)

Najprościej: kliknij dwukrotnie **`start.bat`**. Przy pierwszym uruchomieniu zainstaluje wszystko i otworzy `.env` w Notatniku.
Ustaw hasło, zapisz i uruchom `start.bat` jeszcze raz.

Ręcznie wygląda to tak:

```powershell
npm install
npm run setup        # pobiera ffmpeg, ffprobe i yt-dlp, tworzy plik .env
```

Otwórz plik `.env` i ustaw co najmniej:

```ini
ADMIN_PASSWORD=twoje-dlugie-haslo
SESSION_SECRET=dowolny-dlugi-losowy-ciag
```

Uruchom serwer:

```powershell
npm start
```

Serwer wypisze adresy. Otwórz `http://localhost:8080/admin`, zaloguj się i wklej pierwszy link.
Przycisk „Włącz podgląd” pokazuje kanał tak, jak zobaczą go widzowie.

## Udostępnienie dla VRChat

Aplikacja na pulpit robi to sama (zobacz [Link publiczny](#link-publiczny-wbudowany-tunel) wyżej). Ten rozdział dotyczy serwera Node.js i serwera `vrcinema` bez okna.

Każdy gracz w lobby pobiera obraz bezpośrednio z Twojego serwera, więc kanał musi mieć adres publiczny.

**Najprościej: Cloudflare Tunnel.** Nie wymaga przekierowania portów.

```powershell
winget install --id Cloudflare.cloudflared
cloudflared tunnel --url http://localhost:8080
```

cloudflared wypisze adres w stylu `https://abc-def.trycloudflare.com`. Wtedy:

1. W `.env` ustaw `TRUST_CLOUDFLARE=true` i `PUBLIC_URL=https://abc-def.trycloudflare.com`, potem zrestartuj serwer.
2. Link dla VRChat to `https://abc-def.trycloudflare.com/live/main.m3u8`.
3. Strona zgłoszeń dla znajomych to `https://abc-def.trycloudflare.com/`.

Darmowy szybki tunel zmienia adres po każdym uruchomieniu cloudflared. Stały adres daje nazwany tunel z własną domeną podpiętą do Cloudflare.

**Wskazówki:**
- Link wklejaj w zwykłe pole na link w odtwarzaczu. Specjalne menu typu „Custom Media” w niektórych światach przyjmują tylko własne serwisy.
- Wklej link kanału na [Pastebin](https://pastebin.com) i dawaj znajomym link „raw” do tej wklejki. VRChat sam przejdzie do kanału. Gdy zmieni się adres tunelu, edytujesz tylko wklejkę.

**Ważne w VRChat:** każda osoba w lobby musi włączyć w ustawieniach opcję **Allow Untrusted URLs**.
Twoja domena nie jest na liście zaufanych adresów VRChat, więc bez tej opcji zobaczy czarny ekran. W instancjach Public i Group Public może to nie wystarczyć (od wersji VRChat 1546 decyduje tam lista domen dopuszczonych przez świat; zobacz [uwagę wyżej](#link-publiczny-wbudowany-tunel)).

### Łącze

Każdy widz pobiera osobny strumień. Przy 1080p to około 6 Mb/s wysyłania na osobę, czyli 60 Mb/s dla 10 widzów.
Przy wolniejszym łączu wysyłającym lepszy będzie VPS (rozdział niżej).

## Pierwszy test przejść w VRChat

Ten test sprawdza, czy odtwarzacz AVPro w VRChat płynnie przechodzi między filmami o różnych parametrach.

1. Dodaj do kolejki dwa krótkie filmy o różnej jakości, np. stary film 240p i nowy 1080p60.
2. Obejrzyj w VRChat moment przejścia między nimi oraz przejście do planszy na końcu.
3. Jeśli obraz się zacina, czernieje albo odtwarzacz się rozłącza, zmień w panelu ustawienia i powtórz:
   - najpierw wyłącz „Oznaczaj przejścia między filmami w strumieniu” (**Ustawienia** → **Kanał**),
   - jeśli to nie pomoże, włącz z powrotem i włącz „Zawsze konwertuj obraz”.

„Zawsze konwertuj obraz” mocno obciąża procesor. Aplikacja na pulpit konwertuje na znalezionej karcie graficznej; serwer bez okna potrzebuje w `.env` `VIDEO_ENCODER=h264_nvenc` (albo `h264_qsv`, `h264_amf`).

## Filmy z dysku

W `.env` wpisz foldery z filmami, rozdzielone średnikiem:

```ini
MEDIA_DIRS=D:\Filmy;E:\Seriale
```

Panel serwera bez okna pokazuje wtedy w widoku **Kanał**, pod polem linku, **Pliki z dysku**: wybierz folder z listy, otwieraj podfoldery i dodaj jeden plik (**Dodaj do kolejki**) albo wszystkie filmy z folderu w naturalnej kolejności (odcinek 2 przed 10, **Dodaj cały folder**). To samo robi API panelu (`/api/admin/library`, `/api/admin/channels/main/local`).
Pliki dodaje tylko właściciel. Widzowie widzą wyłącznie tytuł, nigdy ścieżkę. W aplikacji na pulpit przeciągnij pliki albo foldery na okno.

## Ustawienia w panelu

| Ustawienie | Domyślnie |
|---|---|
| Linki od widzów | dowolny serwis (nowa baza); istniejąca baza zachowuje swoją listę w trybie „Tylko dozwolone serwisy” |
| Najdłuższy film od widza | 2 godziny |
| Zgłoszenia z jednego adresu | 3 na 10 minut |
| Filmów w kolejce na osobę | 5 |
| Zatwierdzanie zgłoszeń | wyłączone |

Ty jako właściciel możesz dodawać filmy z dowolnej strony obsługiwanej przez yt-dlp, transmisje na żywo
i całe playlisty YouTube albo listy pixeldrain. Te limity Ciebie nie dotyczą.

### Linki od widzów i sieć domowa

Tryb linków (**Ustawienia** → **Kanał** → **Linki od widzów**; `linkMode` w ustawieniach kanału) to **Dowolny serwis** (`all`; domyślny dla nowej bazy), **Tylko dozwolone serwisy** (`allowlist`: tylko serwisy z listy i ich poddomeny; tak dostaje istniejąca baza, z dotychczasową listą) albo **Wszystkie oprócz zablokowanych** (`blocklist`: każdy serwis poza tymi z listy). Obie listy są zawsze przechowywane; **Dodaj popularne serwisy** wpisuje YouTube, Twitch, Kick, Vimeo, Dailymotion, pixeldrain, CDA, SoundCloud, bilibili, niconico, Streamable, Reddit, X/Twitter i TikTok. W trybie „Tylko dozwolone serwisy” strona zgłoszeń pokazuje listę dozwolonych serwisów.

W każdym trybie link od widza musi być `http` albo `https` i nie może prowadzić do Twojej sieci: odrzucane są `localhost`, `*.local`, `*.lan`, `*.home.arpa`, `*.internal` i nazwy jednowyrazowe, np. `nas`, a także każda nazwa, która rozwiązuje się na adres pętli zwrotnej, prywatny (RFC 1918, `fc00::/7`), link-local, CGNAT (`100.64.0.0/10`), multicast albo inny zarezerwowany (także adres IPv4 zapisany w IPv6). Odrzucana jest też nazwa, której sprawdzenie nie zdążyło się zakończyć; gdy sprawdzenie nie uda się z innego powodu (nazwa nie istnieje, serwer DNS zawiódł), link przechodzi dalej do yt-dlp, który sam zgłasza błąd (sprawdzenie przepuszcza w razie wątpliwości; zobacz DNS rebinding niżej). Sprawdzenie odbywa się, gdy link przychodzi, i jeszcze raz, gdy film startuje (nazwa mogła się zmienić). Adres filmu znaleziony przez yt-dlp jest sprawdzany tak samo, a ffprobe i ffmpeg dostają dokładnie sprawdzony adres, nigdy tekst wypisany przez yt-dlp; adres z ukośnikiem wstecznym, spacją albo znakiem sterującym jest odrzucany. Linki dodane przez właściciela nigdy nie są ograniczane.

Znane ograniczenia (sprawdzany jest adres, nie to, co dzieje się po jego pobraniu):

- Przekierowania wewnątrz yt-dlp i przekierowania HTTP w ffmpeg (publiczny adres filmu, który odpowiada 302 na stronę w Twojej sieci) nie są sprawdzane.
- Playlista albo manifest (HLS, DASH) pod publicznym adresem może wskazywać fragmenty w Twojej sieci; ffmpeg za nimi pójdzie.
- Nazwa, która zmienia odpowiedź między sprawdzeniem a pobraniem (DNS rebinding), nie zostanie wykryta.
- Treść strony w Twojej sieci może wyciec: przez ogólny ekstraktor yt-dlp tytuł albo tekst błędu takiej strony może pojawić się jako tytuł w kolejce albo błąd pozycji.

Jeśli Twoja sieć jest ważniejsza niż wygoda, używaj trybu `allowlist` z serwisami, którym ufasz.

### Pliki wideo i dźwiękowe od widzów

Domyślnie wyłączone: **Ustawienia** → **Kanał** → **Wideo i dźwięk od widzów** → **Pozwól widzom wysyłać małe pliki wideo i dźwiękowe**. Strona zgłoszeń dostaje wtedy pole na plik obok pola linku. Limity (w tym samym bloku): największy plik (50 MB, najwyżej 200 MB), najdłuższy film (10 min), najdłuższy plik dźwiękowy (10 min; obu dotyczy też maksymalna długość kanału) i wszystkie pliki razem (2048 MB). Plik dźwiękowy (muzyka) leci z obrazem zrobionym na jego podstawie, wybranym w **Pliki dźwiękowe** (okładka, fala dźwięku, …). Plik liczy się jak link: sprawdzanie nicku i urządzenia, zablokowane urządzenia, zgłoszenia na adres, filmy w kolejce na osobę i zatwierdzanie zgłoszeń działają tak samo, a kolejka pokazuje go z nickiem widza („wysłany plik”).

- Rozmiar liczy się na bajtach, które naprawdę przychodzą (nie tylko z `Content-Length`); jedno wysyłanie na adres i trzy naraz; każdy kawałek musi przyjść w 30 s, cały plik w 20 min; na dysku zostają 2 GB wolnego miejsca.
- Przechodzą tylko binarne kontenery wideo i dźwięku (MP4/MOV/M4A, WebM/MKV, AVI, FLV, MPEG-TS/PS, Ogg/Opus, WMV, MP3, AAC, FLAC, WAV) rozpoznane po pierwszych bajtach, potem czytane przez ffprobe z wymuszonym formatem i tylko lokalnymi plikami. Playlisty, tekst i obrazy są odrzucane. Okładka w pliku dźwiękowym jest zawsze tylko okładką, nigdy „wideo”: obowiązuje limit długości dźwięku, a okładka musi mieć rozsądny rozmiar. Plik dostaje nazwę nadaną przez serwer (`DATA_DIR/uploads/<losowa>.<rozszerzenie>`); nazwa z przeglądarki służy tylko za tytuł.
- Plik jest usuwany ok. 90 s po tym, jak film poleciał, się nie udał, został odrzucony albo usunięty (także przez **Wyczyść kolejkę** i **Usuń wszystko od tego widza**; ich **Cofnij** działa w tej minucie). Gdy suma jest pełna, najpierw idą pliki, których żaden film już nie potrzebuje, od najstarszych; jeśli to nie wystarczy, wysyłanie jest odrzucane. Wyłączenie przełącznika od razu odrzuca nowe pliki; te, które są już w kolejce, zostają i polecą.
- Przez tunel Cloudflare jedno żądanie może mieć najwyżej 100 MB (limit Cloudflare), więc większe pliki tam nie przejdą; przy szybkim tunelu duże pliki mogą też iść wolno.
Opcje `.env` są opisane w [.env.example](.env.example): język (`LANGUAGE`), enkoder karty graficznej (`VIDEO_ENCODER=h264_nvenc`), cookies dla YouTube (`YTDLP_COOKIES`), klucz API pixeldrain i ścieżki.

## Gdy coś nie działa

- **Film od razu trafia do historii z błędem.** Powód jest przy filmie w historii. yt-dlp aktualizuje się przy każdym starcie i raz na dobę.
- **YouTube każe się zalogować.** Wyeksportuj cookies z przeglądarki do pliku `cookies.txt` i ustaw `YTDLP_COOKIES` w `.env`.
- **pixeldrain zwraca limit pobierania.** Ustaw `PIXELDRAIN_API_KEY` z ustawień swojego konta pixeldrain.
- **Plansza „Buforowanie” trwa długo.** Źródło jest wolne. Po 30 sekundach bez danych film zostaje pominięty.
- **Widzowie nie widzą obrazu.** Sprawdź opcję Allow Untrusted URLs i czy link kanału zaczyna się od `https://`. W instancji Public albo Group Public spróbuj instancji Friends albo Invite (zobacz uwagę o VRChat w [Link publiczny](#link-publiczny-wbudowany-tunel)).

## VPS (Linux, Docker)

W katalogu `deploy` utwórz plik `.env`:

```ini
ADMIN_PASSWORD=twoje-dlugie-haslo
SESSION_SECRET=dowolny-dlugi-losowy-ciag
DOMAIN=tv.twojadomena.pl
```

Skieruj domenę na adres serwera i uruchom:

```bash
cd deploy
docker compose up -d --build
```

Caddy sam pobierze certyfikat HTTPS. Link kanału: `https://tv.twojadomena.pl/live/main.m3u8`.

## Wersja w Rust (eksperymentalna)

W katalogu `engine/` jest druga implementacja tego samego serwera, napisana w Rust. To bezpośredni zamiennik: to samo API HTTP, ten sam plik `.env`, ten sam katalog `data/` i baza. Zamiast procesu Node.js jest jeden mały program, który podczas odtwarzania filmu zajmuje około 22 MB RAM (wersja release; pomiar pamięci roboczej; wersja debug zajmuje 29 MB; ffmpeg to osobny proces i nie jest wliczony).
Wersja Node.js zostaje w repozytorium do czasu przełączenia.

```powershell
cd engine
cargo build --release     # wymaga Rusta; wynik: engine\target\release\vrcinema.exe
cd ..
engine\target\release\vrcinema.exe setup   # pobiera ffmpeg/ffprobe/yt-dlp/Deno (sprawdza sumy kontrolne), tworzy .env
engine\target\release\vrcinema.exe         # uruchamia kanał (z katalogu projektu)
```

Na tym samym katalogu `data/` uruchamiaj naraz tylko jedną wersję. Żeby wrócić, zatrzymaj `vrcinema` i uruchom wersję Node.js (`npm start`); baza jest zgodna w obie strony, z ograniczeniami: wersja Node.js pomija nicki i tryby linków (oraz inne ustawienia z etapu 4) i pokazuje powody zapisane przez serwer Rust (dlaczego film się nie udał albo został pominięty) jako surowy zapisany tekst.
Przełączaj w nowym oknie PowerShell: ustawienia nadpisane wcześniej przez `$env:PORT` czy `$env:DATA_DIR` zostają w starym oknie i wygrywają z `.env`. Wersja Rust blokuje katalog danych (`vrcinema.lock`), więc drugi serwer Rust na tym samym `data/` nie wystartuje.

## Wiersz poleceń (`vrcinema ctl`)

`vrcinema ctl` steruje programem działającym na tym komputerze: serwerem bez okna albo aplikacją na pulpit (program `vrcinema` z `engine/` lub z paczki dla Linuksa; `VRCinema.exe` aplikacji z oknem nie ma konsoli).

```
vrcinema ctl status                     # co jest na antenie, pauza, kolejka
vrcinema ctl queue list                 # film na antenie i kolejka (#numery)
vrcinema ctl queue add https://youtu.be/dQw4w9WgXcQ
vrcinema ctl queue remove 12            # numer z queue list
vrcinema ctl queue clear                # film na antenie gra dalej
vrcinema ctl skip
vrcinema ctl pause | resume
vrcinema ctl seek 1:30                  # od początku; seek +10 / seek -10 od bieżącego miejsca
vrcinema ctl play-now 12
vrcinema ctl settings get               # ustawienia, które ctl może zmienić (lista niżej)
vrcinema ctl settings set oscEnabled true
vrcinema ctl --json status              # odpowiedź programu jako JSON (błędy też: {"ok": false, "error", "code", "exitCode"})
```

- **Jak się loguje.** Przy każdym starcie program zapisuje `ctl.json` w swoim folderze danych (`data/` obok `.env` albo `%LOCALAPPDATA%\VRCinema\data` / `~/.local/share/vrcinema/data` dla aplikacji z oknem) ze swoim lokalnym adresem i nowym losowym tokenem; plik znika, gdy program się zatrzymuje. `ctl` znajduje go sam (`--data-dir <folder>` dla innego folderu). Token działa tylko z tego komputera (`127.0.0.1`/`::1`) i nigdy przez tunel ani reverse proxy; nie otwiera ustawień dostępnych tylko w oknie. Folder danych trzymaj prywatnie (na Linuksie plik może czytać tylko właściciel).
- **Bez pliku** (starszy serwer albo nasłuch tylko na jednym adresie sieciowym) `ctl` loguje się hasłem administratora: `ADMIN_PASSWORD` ze zmiennych środowiska albo `.env`, albo `--password-stdin` (np. `cat haslo.txt | vrcinema ctl --password-stdin status`). Hasło nigdy nie jest przyjmowane jako argument, bo wiersze poleceń widzą inni użytkownicy komputera.
- `--url http://127.0.0.1:8080` wybiera adres; adres na innym komputerze wymaga `--allow-remote` i hasła (token nigdy nie opuszcza komputera).
- **Ustawienia, które ctl może zmienić**: `moderation`, `viewerLists`, `viewerListMax`, `maxDurationS`, `maxActivePerRequester`, `requestLimitCount`, `requestLimitWindowS`, `slateLanguage`, `tickerEnabled`, `oscEnabled`, `oscTemplate`, `oscRepeatS` (wartość jest czytana jako JSON, gdy nim jest: `true`, `30`; inaczej jako tekst). Program sprawdza wartości tak jak w panelu.
- **Kody wyjścia**: `0` gotowe, `1` program odmówił (powód jest wypisany, np. „Teraz nic nie leci.”), `2` złe użycie, `3` nie znaleziono działającego programu albo nie można się z nim połączyć, `4` brak uprawnień (token albo hasło odrzucone). Komunikaty są w języku `LANGUAGE` ze zmiennych środowiska albo `.env`.
- Nie ma polecenia głośności: głośność to własny odtwarzacz gospodarza w VRChat.
## Linux (serwer bez okna)

Serwer `vrcinema` bez okna działa na Linuksie, na x86_64 i na 64-bitowym ARM (np. Raspberry Pi 5). Okna nie ma: kanałem steruje się z panelu WWW, który serwer udostępnia (`http://<serwer>:8080/admin`), z dowolnej przeglądarki, także z telefonu. Obsługa Linuksa jest nowa; zgłaszaj, co nie działa.

### Instalacja

Na Debianie, Ubuntu albo Raspberry Pi OS (64-bit):

```bash
# Kompilacja programu (potrzebny kompilator C dla SQLite i TLS; kilka minut, ok. 30 min na Pi 5)
sudo apt install -y build-essential cmake pkg-config curl git fonts-dejavu-core
curl --proto '=https' -sSf https://sh.rustup.rs | sh -s -- -y && . "$HOME/.cargo/env"
git clone https://github.com/wordoftiger-ux/vrcinema.git
cd vrcinema/engine && cargo build --release

# Osobny użytkownik i katalog
sudo useradd --system --home-dir /opt/vrcinema --shell /usr/sbin/nologin vrcinema
sudo install -d -o vrcinema -g vrcinema /opt/vrcinema
sudo install -m 755 target/release/vrcinema /opt/vrcinema/
sudo install -m 644 ../.env.example /opt/vrcinema/
cd /opt/vrcinema
sudo -u vrcinema ./vrcinema setup      # yt-dlp, ffmpeg, ffprobe, Deno do bin/; tworzy .env
sudo -u vrcinema nano .env            # ADMIN_PASSWORD, SESSION_SECRET, PUBLIC_URL, ...
```

`setup` pobiera linuksowe wersje dla Twojego procesora (yt-dlp, statyczny ffmpeg od BtbN, Deno; `--cloudflared` dokłada cloudflared), sprawdza każdy plik z opublikowaną sumą SHA-256, wznawia przerwane pobieranie i nadaje programom prawo uruchamiania. `setup --force` pobiera je od nowa (aktualizacja). Na innych systemach (32-bitowy ARM, macOS) kończy się wskazówką: zainstaluj ffmpeg, ffprobe i yt-dlp menedżerem pakietów i ustaw `FFMPEG_PATH`, `FFPROBE_PATH` oraz `YTDLP_PATH` w `.env`.

### Uruchamianie jako usługa (systemd)

```bash
sudo cp ~/vrcinema/deploy/vrcinema.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now vrcinema
journalctl -u vrcinema -f             # dziennik (baner pokazuje linki do panelu i kanału)
```

`systemctl stop` / `restart` wysyła SIGTERM: kanał zatrzymuje się porządnie razem ze swoim ffmpeg i yt-dlp. Usługa zabija całą swoją grupę procesów (control group), więc nic nie zostaje nawet po awarii. Działa jako użytkownik `vrcinema` i może zapisywać tylko w `/opt/vrcinema`; `DATA_DIR` albo `MEDIA_DIRS` gdzie indziej muszą być dla niego czytelne (a `DATA_DIR` dopisany do `ReadWritePaths=`).

### Zapora i adres publiczny

- **Panel i strumień**: TCP `PORT` (8080). Za reverse proxy (Caddy, nginx) otwórz tylko 80/443 (`sudo ufw allow 80,443/tcp`), ustaw `TRUST_PROXY=true`, a gdy proxy działa na tej samej maszynie, także `HOST=127.0.0.1`. Bez proxy: `sudo ufw allow 8080/tcp`.
- **Szybki tunel Cloudflare**: `./vrcinema setup --cloudflared`, potem `bin/cloudflared tunnel --url http://localhost:8080`; ustaw `TRUST_CLOUDFLARE=true` i `PUBLIC_URL` (zobacz [Udostępnienie dla VRChat](#udostępnienie-dla-vrchat)). Żaden port nie musi być otwarty. Dla stałego adresu uruchom cloudflared jako osobną usługę (`cloudflared service install`).
- **OBS** (`OBS_STREAM_KEY`): TCP 1935 (`OBS_PORT`). Nasłuchuje tylko na tej maszynie, chyba że `OBS_LAN=1`; wtedy wpuść tylko swój adres (`sudo ufw allow from 203.0.113.7 to any port 1935 proto tcp`). Nigdy nie idzie przez tunel.

### Karta graficzna

`VIDEO_ENCODER=auto` sprawdza przy starcie enkodery sprzętowe w tej kolejności: NVIDIA NVENC (sterownik NVIDIA), Intel Quick Sync, AMD AMF, a potem **VA-API** (`h264_vaapi`: grafika Intel i AMD przez `intel-media-va-driver` albo `mesa-va-drivers`). Wygrywa pierwszy, który zakoduje sekundowy obraz testowy; gdy żaden, używa `libx264` (procesor). Filmy, których nie trzeba konwertować, i tak prawie nie obciążają procesora.

- VA-API potrzebuje dostępu do `/dev/dri`: odkomentuj `SupplementaryGroups=video render` w pliku usługi (albo dodaj swojego użytkownika do grupy `render`). `vainfo` (pakiet `vainfo`) pokazuje, czy sterownik działa.
- Urządzenie to `/dev/dri/renderD128`; przy kilku kartach graficznych ustaw inne przez `VAAPI_DEVICE=/dev/dri/renderD129` w `.env` (aplikacja z oknem: Ustawienia → Narzędzia, „Urządzenie VA-API”).
- `VIDEO_ENCODER=h264_vaapi` wymusza VA-API. Wszystkie kroki programowe (pasek informacyjny, napisy, przejścia, odliczanie, OBS) zostają bez zmian; obraz trafia na kartę graficzną dopiero do kodowania, z klatką kluczową w każdym segmencie.

### Raspberry Pi 5

Pi 5 **nie ma sprzętowego enkodera H.264** (umie dekodować, nie kodować), więc konwertuje na procesorze. VRCinema to rozpoznaje (64-bitowy Linux na ARM z `libx264`) i włącza **lekki profil**: preset `superfast`, domyślnie 3 Mbit/s zamiast 6 i konwertowany obraz najwyżej **720p30**. Filmy, które da się skopiować bez zmian (H.264 do 1080p30), nadal są kopiowane, co prawie nic nie kosztuje. Dziennik mówi o tym przy starcie, a Ustawienia → Zaawansowane pokazują uwagę przy **Maksymalnej rozdzielczości wyjścia** (automatycznie, 1080p, 720p, 480p). Zostaw automatycznie albo 720p; konwersja 1080p na Pi 5 nie nadąża.

**Osobna karta SD.** Jeśli na Pi działa inny system (np. Batocera), zostaw tę kartę bez zmian i zainstaluj Raspberry Pi OS na drugiej karcie (albo dysku USB/NVMe), a potem po prostu zamieniaj karty:

1. Na PC zainstaluj [Raspberry Pi Imager](https://www.raspberrypi.com/software/) i wybierz *Raspberry Pi 5* → **Raspberry Pi OS Lite (64-bit)** (albo wersję z pulpitem) → nową kartę. W ustawieniach (zębatka) ustaw nazwę hosta, użytkownika, hasło i Wi-Fi oraz włącz SSH.
2. Włóż kartę do Pi, uruchom go, odczekaj minutę i połącz się: `ssh <użytkownik>@<nazwa-hosta>.local`.
3. `sudo apt update && sudo apt full-upgrade -y`, potem kroki z [Instalacji](#instalacja) i [Uruchamiania jako usługa](#uruchamianie-jako-usługa-systemd) wyżej (kompilacja trwa ok. 30 minut na Pi 5). `VIDEO_ENCODER=auto` nie znajdzie enkodera sprzętowego i lekki profil włączy się sam.
4. Żeby wrócić do drugiego systemu, wyłącz Pi (`sudo poweroff`) i zamień kartę.

**Czego się spodziewać.**

- Filmy kopiowane: kilka procent procesora. Konwersja 720p30: mniej więcej 2–3 z 4 rdzeni; pasek informacyjny, przejścia albo napisy dokładają swoje. Dwie konwersje naraz (następny film przygotowuje się, gdy leci bieżący) mogą chwilowo zająć wszystkie cztery.
- Użyj oficjalnego **Active Coolera** (albo obudowy z wentylatorem) i oficjalnego zasilacza 27 W: bez chłodzenia Pi 5 zwalnia przy długiej konwersji i obraz się tnie. `vcgencmd measure_temp` pokazuje temperaturę (trzymaj poniżej ok. 80 °C), a `vcgencmd get_throttled` musi zwracać `throttled=0x0`.
- Używaj **64-bitowego** Raspberry Pi OS. Na 32-bitowym nie ma gotowego ffmpeg (`setup` się zatrzyma; użyj ffmpeg z systemu, jak wyżej).
- Trzymaj `DATA_DIR` na dysku SSD/USB zamiast na karcie SD: segmenty są zapisywane bez przerwy.
- Każdy widz pobiera ok. 3 Mbit/s konwertowanego filmu (więcej przy kopiowanym), więc o liczbie widzów decyduje Twoje łącze w górę.

### Aplikacja z oknem na Linuksie

Aplikacja na pulpit (`VRCinema`) ma już linuksowe elementy (dane w `$XDG_DATA_HOME/vrcinema` albo `~/.local/share/vrcinema`, linki otwiera `xdg-open`, bez samoaktualizacji: wydania zawierają tylko `VRCinema.exe`), ale nie była jeszcze budowana ani testowana na Linuksie. Do kompilacji potrzebne są pakiety deweloperskie WebKitGTK (`libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`). Na razie używaj serwera bez okna opisanego wyżej.

## Dla programistów

```powershell
npm test          # testy (część używa prawdziwego ffmpeg z ./bin)
npm run typecheck
```

Architektura i decyzje: `docs/superpowers/specs/2026-10-03-vrcinema-design.md`.
W skrócie: yt-dlp rozpoznaje film, ffmpeg tnie go na 1-sekundowe fragmenty MPEG-TS (bez konwersji, gdy źródło już jest takie, jak po konwersji — H.264 w typowym profilu i poziomie, 8-bit 4:2:0, do 1080p30 ze stałą liczbą klatek, bez przeplotu, SDR, BT.709 albo bez oznaczenia — i ma klatkę kluczową co najwyżej co 1,25 s — ffprobe czyta pakiety pierwszych 6 s; źródło z rzadszymi klatkami kluczowymi, np. większość YouTube, jest konwertowane z klatką kluczową co sekundę),
a nadajnik publikuje je w tempie czasu rzeczywistego i przepisuje znaczniki czasu, żeby filmy i plansze tworzyły jedną oś czasu.
Filmy z TikToka (i każdy film, którego adres strona odrzuca dla ffmpeg kodem HTTP 403/401) yt-dlp najpierw pobiera w całości do `data\temp` (najwyżej 200 MB, w ciągu 100 s; nigdy transmisji na żywo) i stamtąd są odtwarzane; plik znika ok. 3 minuty po ostatnim fragmencie filmu (wznowienie albo przewinięcie korzysta z niego ponownie), a folder jest czyszczony przy każdym starcie. Film pionowy (9:16) jest zawsze konwertowany i wpisywany w kadr 16:9 z czarnymi pasami po bokach.
