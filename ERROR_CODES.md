# ROSE Updater Error Codes

If you see an error code when using the ROSE Online launcher, find it below for help.

---

## Network Errors (ROSE-1xx)

These errors mean the launcher is having trouble connecting to the update server.

### ROSE-100 — Failed to check for updates

The launcher could not reach the update server to see if new updates are available.

**What to try:**
1. Check that your internet connection is working (try opening a website in your browser)
2. Temporarily disable your firewall or antivirus and try again
3. If you are on a work or school network, the network may be blocking the connection — try from a different network
4. Wait a few minutes and try again, the server may be temporarily busy

### ROSE-101 — Could not download update information

The launcher connected to the server but failed to download the list of available updates.

**What to try:**
1. Close the launcher and open it again
2. Check your internet connection is stable (not cutting in and out)
3. If you are using a VPN, try disconnecting it
4. Wait a few minutes and try again

### ROSE-102 — Invalid update server address

The launcher has an incorrect server address configured. This usually means the launcher files are corrupted.

**What to try:**
1. Re-download the launcher from the official ROSE Online website
2. Make sure you are using the latest version of the launcher

### ROSE-103 — Could not set up the download connection

The launcher could not prepare its download system. This is rare and usually caused by system-level issues.

**What to try:**
1. Restart your computer and try again
2. Make sure your operating system is up to date
3. Try running the launcher as administrator (right-click > Run as administrator)

### ROSE-104 — Could not download game data

The launcher connected to the server but could not download a game file. The connection may have dropped mid-download.

**What to try:**
1. Check that your internet connection is stable
2. Close the launcher and open it again — it will resume where it left off
3. If you are on Wi-Fi, try moving closer to your router or using a wired connection
4. Temporarily disable your firewall or antivirus and try again

### ROSE-105 — Failed to download a game file chunk

A small piece of a game file failed to download. This is usually a temporary network hiccup.

**What to try:**
1. Close the launcher and open it again — it will retry the download
2. If this keeps happening, your internet connection may be unstable
3. Try using a wired connection instead of Wi-Fi

---

## File System Errors (ROSE-2xx)

These errors mean the launcher is having trouble reading or writing files on your computer.

### ROSE-200 — Could not create the game folder

The launcher could not create the folder where game files are stored.

**What to try:**
1. Try running the launcher as administrator (right-click > Run as administrator)
2. Make sure the drive where the game is installed has enough free space
3. Check that you have not installed the game in a protected folder (like Program Files) — try installing somewhere else like `C:\Games\ROSE Online`

### ROSE-201 — Could not read local update data

The launcher could not read its saved progress from a previous update.

**What to try:**
1. Close the launcher and open it again
2. Try running the launcher as administrator
3. If this keeps happening, delete the `updater` folder inside your game directory and relaunch — the launcher will re-check all files from scratch

### ROSE-202 — Could not save update progress

The launcher finished downloading but could not save its progress. This means the next launch may re-download some files.

**What to try:**
1. Make sure the game folder is not set to read-only (right-click the folder > Properties > uncheck "Read-only")
2. Make sure the drive has enough free space
3. Try running the launcher as administrator

### ROSE-203 — Could not open game file for reading

The launcher could not open an existing game file to check if it needs updating.

**What to try:**
1. Make sure no other program is using the game files (close the game if it is running)
2. Try running the launcher as administrator
3. Restart your computer and try again

### ROSE-204 — Could not open game file for updating

The launcher downloaded new data but could not write it to a game file.

**What to try:**
1. Make sure no other program is using the game files (close the game if it is running)
2. Check that your antivirus is not blocking the launcher from writing files — add the game folder to your antivirus exceptions
3. Try running the launcher as administrator
4. Make sure the drive has enough free space

### ROSE-205 — Could not create folder

The launcher could not create a subfolder needed for the game.

**What to try:**
1. Try running the launcher as administrator
2. Make sure the drive has enough free space
3. Check that the game path does not contain special characters

### ROSE-206 — Could not write update to disk

The launcher downloaded new data but could not save it to your drive.

**What to try:**
1. Make sure the drive has enough free space (you need at least 2 GB free)
2. Make sure no other program is using the game files
3. Try running the launcher as administrator
4. If you are installing to an external or network drive, try installing to your main drive instead

### ROSE-207 — Could not check game file

The launcher could not read information about a game file on your computer.

**What to try:**
1. Close the launcher and open it again
2. Try running the launcher as administrator
3. If this keeps happening, try deleting the specific file mentioned in the error details — the launcher will re-download it

---

## Data Integrity Errors (ROSE-3xx)

These errors mean something went wrong with the downloaded data — it may be corrupted or incomplete.

### ROSE-300 — Received invalid data from the update server

The server sent data that the launcher could not understand. This could mean the server is having issues or something is interfering with the download.

**What to try:**
1. Wait a few minutes and try again — the server may be in the middle of publishing an update
2. If you are using a VPN or proxy, try disabling it
3. Clear your DNS cache: open Command Prompt and type `ipconfig /flushdns`, then try again

### ROSE-301 — Downloaded data appears corrupted

A downloaded file could not be decompressed. The data was damaged during transfer.

**What to try:**
1. Close the launcher and open it again — it will re-download the corrupted file
2. If this keeps happening, your internet connection may be unreliable
3. Try using a wired connection instead of Wi-Fi
4. Temporarily disable your antivirus — some antivirus programs modify downloaded files

### ROSE-302 — Downloaded data failed integrity check

A downloaded file does not match what the server expected. The data was changed during transfer.

**What to try:**
1. Close the launcher and open it again — it will re-download the file
2. If this keeps happening, check if your antivirus or firewall is scanning/modifying downloads
3. Try disabling any VPN or proxy
4. Try using a different internet connection

### ROSE-303 — Could not prepare game file for updating

The launcher could not reorganize an existing game file to prepare it for an update.

**What to try:**
1. Close the launcher and open it again
2. Make sure no other program is using the game files
3. Try deleting the specific file mentioned in the error details — the launcher will re-download it completely

### ROSE-304 — Could not verify a local game file

The launcher could not read and verify an existing game file on your computer.

**What to try:**
1. Close the launcher and open it again
2. The file may be corrupted — try deleting it and relaunching so it gets re-downloaded
3. Run a disk check on your drive to make sure there are no hardware issues

### ROSE-305 — Could not process downloaded data

A background download task failed unexpectedly.

**What to try:**
1. Close the launcher and open it again
2. If this keeps happening, try restarting your computer
3. Make sure your system has enough free memory (close other programs)

---

## Launcher Errors (ROSE-4xx)

These errors are related to the launcher application itself.

### ROSE-400 — Could not remove the old launcher file

The launcher updated itself but could not clean up the old version.

**What to try:**
1. Close the launcher and open it again
2. Try running the launcher as administrator
3. Make sure no other program is using the launcher file

### ROSE-401 — Could not replace the launcher file

The launcher downloaded a new version of itself but could not swap it in.

**What to try:**
1. Close the launcher and open it again
2. Try running the launcher as administrator
3. Check that your antivirus is not blocking the launcher from modifying itself — add the launcher to your antivirus exceptions

### ROSE-402 — Could not determine the launcher location

The launcher could not figure out where it is installed on your computer. This is very rare.

**What to try:**
1. Make sure the launcher is not running from a temporary or network location
2. Try moving the game folder to a simpler path like `C:\Games\ROSE Online`
3. Re-download the launcher from the official website

### ROSE-403 — Could not restart the launcher after updating

The launcher updated itself but could not start the new version.

**What to try:**
1. Manually close and reopen the launcher
2. Try running the launcher as administrator
3. Check that your antivirus is not blocking the launcher

### ROSE-404 — Could not start the game

The launcher tried to start the game but could not find or run the game executable.

**What to try:**
1. Make sure the game has finished downloading (wait for the progress bar to complete)
2. Check that your antivirus has not quarantined the game file — look for `trose.exe` in your game folder
3. Try running the launcher as administrator
4. If `trose.exe` is missing, close and reopen the launcher to re-download it

---

## Initialization Errors (ROSE-5xx)

These errors happen when the launcher is starting up.

### ROSE-500 — Failed to initialize logging

The launcher could not set up its log file. The launcher may still work, but crash details will not be saved.

**What to try:**
1. Make sure your user account has permission to write to the AppData folder
2. Try running the launcher as administrator
3. Check that your drive has free space

---

## Still having trouble?

If none of the steps above fix your issue:

1. Find your log file at `%LOCALAPPDATA%\Rednim Games\ROSE Online\rose-updater.log` (paste this path into Windows Explorer)
2. Share the log file and the error code with the support team on Discord
