driver install cmd:
.\target\release\server.exe --install-driver


adb:
$adb = "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe"
& $adb devices
& $adb install -r android\app\build\outputs\apk\debug\app-debug.apk

