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
