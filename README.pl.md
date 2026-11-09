<p align="center">
  <img src="desktop/icons/icon.png" alt="VRCinema" width="128">
</p>

<h1 align="center">VRCinema</h1>

<p align="center"><b>Twoje własne kino w VRChat.</b><br>
Wklej link albo upuść plik wideo, a cała instancja ogląda to razem, w tej samej chwili.</p>

<p align="center"><a href="README.md">English</a> · <a href="docs/manual.pl.md">Pełna instrukcja</a></p>

---

## Co to jest?

VRCinema to program na Windows, który zamienia filmy wybrane przez **Ciebie** w jeden niekończący się kanał na żywo. Link dla VRChat podajesz raz. Potem tylko dodajesz filmy do kolejki, a one lecą jeden po drugim, dla wszystkich, równo.

- **Ty** dodajesz filmy z YouTube i wielu innych stron albo z własnego dysku.
- **Znajomi** też mogą coś zaproponować, z prostej strony w przeglądarce (pod nickiem, a jeśli chcesz, za Twoją zgodą).
- **Odtwarzacz wideo w świecie** VRChat gra link kanału. Nigdy nie trzeba go zmieniać.

Gdy kolejka jest pusta, kanał pokazuje spokojny ekran oczekiwania (z kodem QR do strony zgłoszeń), więc odtwarzacz w VRChat się nie rozłącza.

## Jak zacząć

1. **Pobierz** `VRCinema.exe` ze strony [Releases](https://github.com/wordoftiger-ux/VRCinema/releases). Bez instalatora: wrzuć plik gdziekolwiek i uruchom. (Windows 10/11, 64-bit.)
2. **Przejdź krótką konfigurację.** Program pobierze potrzebne narzędzia, wybierze najlepszy dla Twojego komputera koder wideo i uruchomi kanał. Każdy krok możesz pominąć, domyślne ustawienia są dobre.
3. **Skopiuj link dla VRChat** na końcu konfiguracji (potem: przycisk **Kopiuj link** na dole okna).
4. **Wklej go** do odtwarzacza wideo w świecie VRChat.
5. **Dodawaj filmy**: wklej link do okna (albo naciśnij <kbd>Ctrl</kbd>+<kbd>V</kbd>) albo przeciągnij na nie pliki wideo.

> Windows może napisać „System Windows ochronił ten komputer", bo plik nie jest podpisany. Kliknij **Więcej informacji**, a potem **Uruchom mimo to**.

## Co potrafi

**Prowadzenie seansu**
- **Podgląd** filmu, nad którym pracujesz (reaguje od razu: pauza, przewijanie, pomijanie) i **antena** pokazująca to, co widzą widzowie, kilka sekund później.
- Kolejka z „zagraj jako następny", przycinaniem początku i końca filmu, startem o wybranej godzinie i odliczaniem przed filmem.
- Usuwanie wszystkiego, co dodał dany widz, albo blokada tej osoby.
- Mały odtwarzacz zostaje z Tobą po przejściu do innych zakładek.

**Zaproś znajomych**
- Strona zgłoszeń dla widzów: dodają linki (albo małe pliki wideo i muzyczne) pod nickiem chronionym PIN-em.
- Limity, zatwierdzanie, playlisty od widzów i lista blokad: Ty decydujesz, jak otwarte to ma być.

**Dobry wygląd**
- Pasek informacyjny w obrazie: tytuł, kto dodał, co dalej, Twoje własne wiadomości, kod QR i pasek postępu.
- Plansze na przerwy i oczekiwanie, z własnym obrazem, jeśli chcesz. Muzyka dostaje okładkę i ruchome słupki.
- Opcjonalnie płynne przejścia, wyrównywanie głośności i napisy.

**Więcej możliwości**
- Wbudowany **link publiczny**: bez konta i bez konfiguracji. Stały link na własnej domenie jest opcjonalny.
- **OBS** może nadawać na kanał (i ma priorytet nad kolejką).
- Pokazywanie okna, ekranu albo kamery z tego komputera.
- **Panel zdalny** do prowadzenia kanału z telefonu i krótki tekst „teraz gra" na czacie VRChat.
- Sześć języków: angielski, polski, hiszpański, niemiecki, francuski i japoński (beta).

## Warto wiedzieć

- To Twój komputer wysyła wideo, więc liczy się **prędkość wysyłania** (upload): im więcej osób ogląda, tym więcej potrzeba. Program ostrzega, gdy łącze wygląda na za słabe.
- Większość filmów jest tylko przepakowywana, a nie konwertowana, więc obciążenie komputera jest niewielkie.
- Wszystko jest w `%LOCALAPPDATA%\VRCinema`. Usunięcie tego folderu resetuje program.

## Bez okna

Na Linuksie jest też serwer bez okna z tym samym panelem. Zobacz [instrukcję](docs/manual.pl.md#linux-serwer-bez-okna).

## Używaj rozsądnie

VRCinema służy do oglądania **prywatnie ze znajomymi**. Kto uruchamia program, odpowiada za to, co jest nadawane. Rozpowszechnianie filmów, do których nie masz praw, może naruszać prawo autorskie i regulaminy stron, z których pochodzą.

## Więcej

- [Pełna instrukcja](docs/manual.pl.md) z każdym ustawieniem, rozwiązywaniem problemów i szczegółami technicznymi.
- Licencja: [MIT](LICENSE).
