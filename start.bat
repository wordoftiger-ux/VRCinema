@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul

where node >nul 2>nul
if errorlevel 1 (
  echo Node.js is not installed. Download it from https://nodejs.org ^(version 22.18 or newer^).
  echo Brak Node.js. Pobierz go ze strony https://nodejs.org ^(wersja 22.18 lub nowsza^).
  pause
  exit /b 1
)

if not exist node_modules (
  echo Installing dependencies / Instalowanie zaleznosci...
  call npm install --omit=dev
  if errorlevel 1 goto failed
)

if not exist bin\ffmpeg.exe goto setup
if not exist bin\yt-dlp.exe goto setup
goto checkenv

:setup
echo Downloading ffmpeg and yt-dlp / Pobieranie ffmpeg i yt-dlp...
call npm run setup
if errorlevel 1 goto failed

:checkenv
if not exist .env copy .env.example .env >nul
findstr /r /c:"^ADMIN_PASSWORD=zmien-mnie" .env >nul
if not errorlevel 1 (
  echo.
  echo Set ADMIN_PASSWORD ^(at least 8 characters^) in .env, save the file and run start.bat again.
  echo Ustaw ADMIN_PASSWORD ^(co najmniej 8 znakow^) w pliku .env, zapisz go i uruchom start.bat ponownie.
  start "" notepad .env
  pause
  exit /b 0
)

call npm start
pause
exit /b 0

:failed
echo Something went wrong / Cos poszlo nie tak.
pause
exit /b 1
