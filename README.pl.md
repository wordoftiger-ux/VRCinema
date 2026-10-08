# Kanał filmowy dla VRChat

Serwer, który z linków do YouTube, pixeldrain, CDA i innych stron robi **jeden ciągły kanał telewizyjny HLS**.
Link kanału wklejasz raz do odtwarzacza wideo w świecie VRChat, a filmy z kolejki lecą po kolei.
Wszyscy w lobby widzą ten sam moment filmu.

- Panel właściciela (`/admin`) z hasłem: dodawanie filmów i playlist, kolejność, pomijanie, podgląd, ustawienia.
- Publiczna strona zgłoszeń (`/`): widzowie dodają filmy, z limitami i opcjonalnym zatwierdzaniem.
- Gdy kolejka jest pusta, kanał nadaje planszę „Brak filmów w kolejce” i nie zrywa połączenia.
- Możesz puszczać też filmy z własnego dysku.

English version: [README.md](README.md).

> **Program jest do prywatnego oglądania ze znajomymi.** Za puszczane treści odpowiada osoba, która uruchamia serwer.
> Retransmisja cudzych utworów bez praw może naruszać prawo autorskie i regulaminy serwisów, z których pochodzą.

## Aplikacja na pulpit (Windows)

`VRCinema.exe` to cały serwer w jednym oknie: bez Node.js, bez instalatora, bez pliku `.env`.

- **Pobranie**: `VRCinema.exe` ze strony [Releases](https://github.com/wordoftiger-ux/vrcinema/releases) (około 19 MB). Połóż go gdziekolwiek, np. na pulpicie. Windows 10/11, 64-bitowy; potrzebny jest składnik Microsoft Edge WebView2 Runtime, który jest w Windows 11 i w większości instalacji Windows 10 (jeśli go brakuje, program podpowie, skąd go wziąć).
- **Pierwsze uruchomienie**: kreator pobiera narzędzia (ffmpeg, yt-dlp, środowisko Deno potrzebne dla YouTube i cloudflared do linku publicznego, razem około 300 MB) do `%LOCALAPPDATA%\VRCinema`, sprawdza kartę graficzną i wybiera enkoder (NVIDIA, Intel, AMD albo procesor), uruchamia kanał i pokazuje jego link publiczny. Potem program od razu otwiera panel.
- **SmartScreen**: plik nie jest podpisany certyfikatem, więc Windows może pokazać „System Windows chronił ten komputer”. Kliknij **Więcej informacji**, potem **Uruchom mimo to**.
- **Dodawanie filmów**: przeciągnij pliki wideo lub całe foldery na okno albo wklej linki w panelu. Przeciągnięte pliki są odtwarzane z miejsca, w którym leżą na dysku.
- **Zasobnik**: zamknięcie okna zostawia kanał w zasobniku (za pierwszym razem pokaże się dymek). Menu zasobnika otwiera panel, kopiuje link kanału, pomija bieżący film i kończy program. **Zakończ** zatrzymuje kanał i wszystkie procesy ffmpeg.
- **Panel zdalny**: domyślnie wyłączony; panel jest dostępny tylko z tego komputera. Aby zarządzać kanałem z innego urządzenia, włącz go w zakładce **Ustawienia** i ustaw hasło.
- **Aktualizacje**: program sprawdza, czy jest nowszy `VRCinema.exe`, przy starcie i co 24 godziny, i proponuje instalację (plik jest weryfikowany sumą SHA-256, a stary zostaje, dopóki nowy się nie uruchomi). Wymaga to publicznych wydań w repozytorium.
- **Dane**: wszystko leży w `%LOCALAPPDATA%\VRCinema` (`config.json`, `tools\`, `data\`, `logs\`). Zmienna `VRCINEMA_HOME` wskazuje inny folder. Usunięcie folderu przywraca stan początkowy.
- **Rozmiar i pamięć** (zmierzone, wersja release): `VRCinema.exe` ma 19,4 MB. Przy otwartym oknie sam zajmuje około 40 MB, a procesy WebView2 rysujące okno około 330 MB; cloudflared zajmuje około 40 MB, a ffmpeg to osobny proces.
- **Bez okna** (serwer, Docker): zobacz [VPS (Linux, Docker)](#vps-linux-docker) i [Wersja w Rust](#wersja-w-rust-eksperymentalna) niżej; serwer `vrcinema` bez okna działa jak dotąd.

### Link publiczny (wbudowany tunel)

Program sam udostępnia kanał w internecie: bez konta, bez terminala i bez wpisywania adresu.

- **Szybki tunel, domyślnie włączony**: przy starcie program uruchamia własną kopię [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) i dostaje adres w stylu `https://abc-def.trycloudflare.com`. Znacznik u góry panelu pokazuje **W sieci** i przycisk kopiujący link kanału (`<adres>/live/main.m3u8`); link kopiuje też menu zasobnika.
- **Adres zmienia się przy każdym uruchomieniu programu** (i gdy tunel musi połączyć się od nowa). Podaj znajomym nowy link albo ustaw stały link (niżej).
- **Zastępuje własny cloudflared.** Jeśli przy poprzedniej wersji uruchamiałeś sam `cloudflared tunnel --url …`, nie musisz już tego robić. Przy włączonym wbudowanym tunelu ręcznie wpisany adres publiczny (**Ustawienia** → **Zaawansowane**) i zaufanie do Cloudflare nie są używane; działają tylko po wyłączeniu tunelu w **Ustawieniach** → **Internet**. Konfiguracja z poprzedniej wersji też dostaje włączony tunel, a brakujący cloudflared program pobiera sam w tle (gdy się nie uda, np. bez internetu, próbuje ponownie co 5 minut).
- **Kod QR na planszy oczekiwania**: gdy kolejka jest pusta, kanał pokazuje „Brak filmów w kolejce”, kod QR strony zgłoszeń i pod nim jej adres. Znajomi w VRChat skanują go telefonem i dodają film. Nowy adres podmienia kod na żywo, bez restartu. Bez adresu plansza mówi „Łączenie z internetem…”.
- **Wyłączenie**: **Ustawienia** → **Internet** → „Udostępniaj kanał w internecie”. Bez tunelu kanał jest dostępny tylko z tego komputera (albo pod adresem wpisanym ręcznie).

### Stały link (własna domena)

Link, który się nie zmienia, wymaga darmowego konta [Cloudflare](https://dash.cloudflare.com/sign-up) i własnej domeny:

1. Kup domenę u dowolnego rejestratora (wystarczy tania) i [dodaj ją do konta Cloudflare](https://developers.cloudflare.com/fundamentals/setup/manage-domains/add-site/): Cloudflare poda dwa serwery nazw, które ustawiasz u rejestratora. Poczekaj, aż Cloudflare pokaże domenę jako **Active**.
2. W VRCinema otwórz **Ustawienia** → **Internet** → **Ustaw stały link**. Program raz otworzy przeglądarkę: zaloguj się do Cloudflare i zatwierdź domenę. Program czeka na logowanie 5 minut; **Anuluj** je przerywa, a po przerwaniu albo upływie czasu **Spróbuj ponownie** zaczyna od nowa. Na koncie bez domeny logowanie nigdy się nie kończy (najpierw dodaj domenę, krok 1).
3. Wpisz pełną nazwę hosta dla kanału, np. `kanal.example.com`. Program tworzy tunel o nazwie `vrcinema` i rekord DNS dla tej nazwy. Jeśli nazwa ma już rekord, zapyta, czy go nadpisać, czy wybrać inną nazwę.

Od tej chwili link kanału to `https://kanal.example.com/live/main.m3u8`, także po restarcie i zmianie portu. **Wróć do szybkiego tunelu** przełącza z powrotem na szybki tunel (stały link zostaje ustawiony); **Usuń stały link** usuwa tunel (jeśli rekord DNS nadal istnieje, usuń go w panelu Cloudflare). Certyfikat Cloudflare i poświadczenia tunelu leżą w `%LOCALAPPDATA%\VRCinema\tunnel\` i nigdy nie trafiają do dziennika ani na ekran.

### Automatyczna pauza

Widzowie nie tracą kawałka filmu, gdy zerwie się połączenie:

- **Utrata internetu** (domyślnie włączona): gdy tunel nie działa przez 10 sekund, film się zatrzymuje, a kanał pokazuje „Pauza — wracamy za chwilę”. Po powrocie tunelu film wznawia się od tego samego miejsca. Krótsze przerwy niczego nie wstrzymują.
- **Nikt nie ogląda** (domyślnie wyłączona): po 30 sekundach bez widzów film się zatrzymuje i wznawia, gdy tylko ktoś zacznie oglądać (podgląd w panelu też liczy się jako widz).
- Transmisje na żywo nigdy nie są wstrzymywane. Obie opcje są w **Ustawieniach** → **Internet**. Wznowiony film rusza po kilku sekundach planszy buforowania; tak ma być.

### Lista testów ręcznych dla właściciela

Tego nie sprawdzają testy automatyczne, potrzebna jest osoba przy ekranie:

- [ ] Czyste konto Windows (albo komputer, na którym programu nigdy nie było): pierwsze uruchomienie z pustego folderu, kreator, pierwszy film leci.
- [ ] Przeciąganie od początku do końca: jeden plik, kilka plików, folder, plik niebędący filmem (pomijany); strona pokazuje wynik, film leci.
- [ ] Podążanie za zmianą portu: zmień port w **Ustawieniach**; okno przeładowuje się pod nowym adresem.
- [ ] Wybór pliku cookies: wskaż plik w **Ustawieniach** i dodaj link, który go wymaga.
- [ ] Autostart włącz/wyłącz: z Windows program startuje w zasobniku bez okna; wyłączenie usuwa wpis.
- [ ] Restart po aktualizacji: zainstaluj nowsze wydanie podstawione lokalnie; program uruchamia się ponownie, a stary plik znika.
- [ ] Kliknięcia w linki: zewnętrzne linki w panelu otwierają się w domyślnej przeglądarce, okno zostaje przy programie.
- [ ] Pozycje menu zasobnika: otwórz panel, kopiuj link, pomiń, zakończ; dwuklik w ikonę.
- [ ] Dymek przy zamknięciu do zasobnika: zamknięcie okna chowa je i pokazuje komunikat raz.
- [ ] Drugie uruchomienie: przy działającym programie (okno schowane do zasobnika albo zminimalizowane) uruchom `VRCinema.exe` jeszcze raz; istniejące okno wraca na wierzch i nie startuje druga kopia (także po **Zakończ**: nowe uruchomienie startuje normalnie w ciągu kilku sekund).
- [ ] Panel zdalny: w **Ustawieniach** ustaw hasło i włącz panel zdalny (z dostępem z sieci albo adresem publicznym); zaloguj się z innego urządzenia (np. telefonu) pod `/admin`; złe hasło jest odrzucane; wyłączenie panelu albo zmiana hasła wylogowuje to urządzenie.
- [ ] Pasek naprawy: przy uszkodzonych narzędziach (np. zamień `tools\ffmpeg.exe` w folderze danych na pusty plik i uruchom program ponownie) panel pokazuje „Kanał nie wystartował…” z przyciskami **Uruchom ponownie kanał** i **Napraw narzędzia**; naprawa przywraca kanał.
- [ ] Stały link od początku do końca z prawdziwą domeną: **Ustaw stały link** raz otwiera przeglądarkę do logowania w Cloudflare; potem pojawia się krok z nazwą hosta; kanał działa pod `https://<host>/live/main.m3u8` i zachowuje ten link po restarcie programu i zmianie portu. Zakończenie VRCinema (zasobnik → **Zakończ**) w trakcie lub po logowaniu nie zamyka przeglądarki. Nazwa hosta, która ma już rekord DNS, pokazuje pytanie o nadpisanie, a nadpisanie działa. **Wróć do szybkiego tunelu** i **Usuń stały link** działają (tunel znika z panelu Cloudflare).
- [ ] Kod QR z VRChat: przy pustej kolejce zeskanuj telefonem kod QR z planszy oczekiwania w świecie VRChat; otwiera się strona zgłoszeń, a dodany tam film leci.
- [ ] Automatyczna pauza: w trakcie filmu odłącz kabel sieciowy (albo wyłącz Wi-Fi) na co najmniej 15 s; znacznik pokazuje „Brak internetu — kanał wstrzymany”, a film się zatrzymuje; po podłączeniu tunel wraca (z tym samym adresem, jeśli cloudflared działał dalej, z nowym, jeśli musiał uruchomić się od nowa), a film leci dalej od tego samego miejsca.
- [ ] Aktualizacja z poprzedniej wersji: uruchom nowy `VRCinema.exe` na folderze danych poprzedniej wersji (jej `config.json`); tunel jest włączony; program sam pobiera cloudflared w tle; panel pokazuje pobieranie (pasek „Pobieranie programu cloudflared…”, nigdy „kanał nie nadaje”), a potem link publiczny; film może się zatrzymać, jeśli pobieranie trwa dłużej niż 10 s.

## Wymagania

- Windows 10/11, Linux albo macOS.
- [Node.js](https://nodejs.org) w wersji **22.18 lub nowszej** (zalecana 24).
- Do udostępnienia w internecie: [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) albo VPS.

ffmpeg i yt-dlp instalują się automatycznie do folderu `bin`.

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
Twoja domena nie jest na liście zaufanych adresów VRChat, więc bez tej opcji zobaczy czarny ekran.

### Łącze

Każdy widz pobiera osobny strumień. Przy 1080p to około 6 Mb/s wysyłania na osobę, czyli 60 Mb/s dla 10 widzów.
Przy wolniejszym łączu wysyłającym lepszy będzie VPS (rozdział niżej).

## Pierwszy test przejść w VRChat

Ten test sprawdza, czy odtwarzacz AVPro w VRChat płynnie przechodzi między filmami o różnych parametrach.

1. Dodaj do kolejki dwa krótkie filmy o różnej jakości, np. stary film 240p i nowy 1080p60.
2. Obejrzyj w VRChat moment przejścia między nimi oraz przejście do planszy na końcu.
3. Jeśli obraz się zacina, czernieje albo odtwarzacz się rozłącza, zmień w panelu ustawienia i powtórz:
   - najpierw wyłącz „Oznaczaj przejścia między filmami”,
   - jeśli to nie pomoże, włącz z powrotem i włącz „Zawsze konwertuj obraz”.

„Zawsze konwertuj obraz” mocno obciąża procesor. Z kartą NVIDIA ustaw w `.env` `VIDEO_ENCODER=h264_nvenc`, a konwersja przejdzie na kartę graficzną.

## Filmy z dysku

W `.env` wpisz foldery z filmami, rozdzielone średnikiem:

```ini
MEDIA_DIRS=D:\Filmy;E:\Seriale
```

W panelu pojawi się sekcja „Pliki z dysku”. Przeglądasz w niej foldery i dodajesz do kolejki pojedynczy plik albo cały folder.
Pliki z folderu trafiają do kolejki w naturalnej kolejności, czyli odcinek 2 przed 10.
Pliki dodaje tylko właściciel. Widzowie widzą wyłącznie tytuł, nigdy ścieżkę.

## Ustawienia w panelu

| Ustawienie | Domyślnie |
|---|---|
| Dozwolone serwisy dla widzów | youtube.com, youtu.be, pixeldrain.com, cda.pl |
| Najdłuższy film od widza | 2 godziny |
| Zgłoszenia z jednego adresu | 3 na 10 minut |
| Filmów w kolejce na osobę | 5 |
| Zatwierdzanie zgłoszeń | wyłączone |

Ty jako właściciel możesz dodawać filmy z dowolnej strony obsługiwanej przez yt-dlp, transmisje na żywo
i całe playlisty YouTube albo listy pixeldrain. Te limity Ciebie nie dotyczą.

## Gdy coś nie działa

- **Film od razu trafia do historii z błędem.** Powód jest przy filmie w historii. yt-dlp aktualizuje się przy każdym starcie i raz na dobę.
- **YouTube każe się zalogować.** Wyeksportuj cookies z przeglądarki do pliku `cookies.txt` i ustaw `YTDLP_COOKIES` w `.env`.
- **pixeldrain zwraca limit pobierania.** Ustaw `PIXELDRAIN_API_KEY` z ustawień swojego konta pixeldrain.
- **Plansza „Buforowanie” trwa długo.** Źródło jest wolne. Po 30 sekundach bez danych film zostaje pominięty.
- **Widzowie nie widzą obrazu.** Sprawdź opcję Allow Untrusted URLs i czy link kanału zaczyna się od `https://`.

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
engine\target\release\vrcinema.exe setup   # pobiera ffmpeg/ffprobe/yt-dlp, tworzy .env
engine\target\release\vrcinema.exe         # uruchamia kanał (z katalogu projektu)
```

Na tym samym katalogu `data/` uruchamiaj naraz tylko jedną wersję. Żeby wrócić, zatrzymaj `vrcinema` i uruchom wersję Node.js (`npm start`); baza jest zgodna w obie strony.
Przełączaj w nowym oknie PowerShell: ustawienia nadpisane wcześniej przez `$env:PORT` czy `$env:DATA_DIR` zostają w starym oknie i wygrywają z `.env`. Wersja Rust blokuje katalog danych (`vrcinema.lock`), więc drugi serwer Rust na tym samym `data/` nie wystartuje.

## Dla programistów

```powershell
npm test          # testy (część używa prawdziwego ffmpeg z ./bin)
npm run typecheck
```

Architektura i decyzje: `docs/superpowers/specs/2026-10-03-vrcinema-design.md`.
W skrócie: yt-dlp rozpoznaje film, ffmpeg tnie go na 4-sekundowe fragmenty MPEG-TS (bez konwersji, gdy źródło to H.264 do 1080p),
a nadajnik publikuje je w tempie czasu rzeczywistego i przepisuje znaczniki czasu, żeby filmy i plansze tworzyły jedną oś czasu.
