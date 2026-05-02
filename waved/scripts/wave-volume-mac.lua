-- Hammerspoon shim — make Mac volume / mute keys control the Wave.
--
-- Why: macOS doesn't dispatch UAC2 SET_CUR(VOLUME) for the Wave's
-- playback Feature Unit (kernel f_uac2 5.4 doesn't expose a Master
-- channel control that USBAudio.kext keys on), and silently no-ops the
-- F11/F12 keys. This shim catches the system-defined media keys and
-- pushes the change through `adb shell amixer` instead, hitting numid=2
-- (PCM Playback Volume) and numid=1 (PCM Playback Switch) — which
-- waved's UacControl already polls and routes onto the SP#6 follow-up
-- gain stage.
--
-- Setup:
--   1. Install Hammerspoon: brew install --cask hammerspoon
--      (or download from https://www.hammerspoon.org/)
--   2. Copy this file to ~/.hammerspoon/init.lua, OR include it from an
--      existing init.lua via:  dofile("/path/to/wave-volume-mac.lua")
--   3. Open Hammerspoon → Preferences → Reload Config (or restart)
--   4. Grant Accessibility permission when prompted (System Settings →
--      Privacy & Security → Accessibility → enable Hammerspoon)
--   5. Make sure `adb` is in your PATH (`which adb` should print a
--      path; if not, set ADB_PATH below to the full path).
--
-- Behaviour:
--   - Only intercepts F10/F11/F12 when Wave is the active output
--     device. With other outputs (built-in speakers, AirPods, etc.) the
--     keys fall through to macOS's normal handler.
--   - F12 / F11 step Wave's volume by VOLUME_STEP (default 6%).
--   - F10 toggles Wave's playback switch directly (which fires
--     `host set speaker mute → true/false` in the waved log).
--   - Brief HUD overlay shows the new state.

local ADB_PATH      = "/usr/local/bin/adb"  -- fall back to /opt/homebrew/bin/adb on Apple Silicon if needed
local VOLUME_STEP   = 6
local VOLUME_MIN    = 0
local VOLUME_MAX    = 100
local INITIAL_VOL   = 50  -- best-effort cache; first press syncs Wave to this

-- Local cache; we don't read back over adb on every keystroke (would add
-- ~80 ms latency). Wave's actual state may briefly diverge after a touch-UI
-- adjustment; the next key press realigns.
local waveVolume = INITIAL_VOL
local waveMuted  = false

-- Resolve adb path. Prefer the configured one; fall back to looking it up.
local function adbBinary()
    if hs.fs.attributes(ADB_PATH) then return ADB_PATH end
    -- Try Apple Silicon Homebrew location.
    if hs.fs.attributes("/opt/homebrew/bin/adb") then return "/opt/homebrew/bin/adb" end
    return "adb"  -- final fallback: rely on PATH
end

-- Fire-and-forget shell exec via adb. Suppresses stdout/stderr so we don't
-- leak amixer chatter into Hammerspoon's console.
local function adbShell(cmd)
    hs.task.new(adbBinary(), nil, {"shell", cmd .. " >/dev/null 2>&1"}):start()
end

local function isWaveOutput()
    local dev = hs.audiodevice.defaultOutputDevice()
    if dev == nil then return false end
    local name = dev:name() or ""
    return name:find("Wave")    ~= nil
        or name:find("Insta360") ~= nil
        or name:find("UAC2")     ~= nil
        or name:find("psp")      ~= nil  -- product string from S89usbgadget
end

local function setWaveVolume(v)
    v = math.max(VOLUME_MIN, math.min(VOLUME_MAX, v))
    waveVolume = v
    -- numid=2 is "PCM Playback Volume" on hw:1 — UacControl translates
    -- the resulting kernel mixer event into Command::SetSpeakerVolume.
    adbShell(string.format("amixer -c 1 cset numid=2 %d", v))
    hs.alert.closeAll()
    hs.alert.show(string.format("Wave: %d%%", v), {textSize = 24}, 0.6)
end

local function toggleWaveMute()
    waveMuted = not waveMuted
    -- numid=1 is "PCM Playback Switch". UacControl reads it and dispatches
    -- ToggleSpeakerMute via the speaker-mute branch (not the redirect path).
    local switch = waveMuted and "off" or "on"
    adbShell(string.format("amixer -c 1 cset numid=1 %s", switch))
    hs.alert.closeAll()
    hs.alert.show(waveMuted and "Wave: muted" or "Wave: unmuted",
                  {textSize = 24}, 0.6)
end

-- Tap the system-defined media key stream (these are how F10/F11/F12 and
-- the Touch Bar volume controls all surface on macOS).
local mediaKeyTap = hs.eventtap.new(
    {hs.eventtap.event.types.systemDefined},
    function(event)
        local data = event:systemKey()
        if data == nil or data.down ~= true then return false end
        if not isWaveOutput() then return false end

        if data.key == "SOUND_UP" then
            setWaveVolume(waveVolume + VOLUME_STEP)
            return true   -- consume so macOS doesn't also try to handle it
        elseif data.key == "SOUND_DOWN" then
            setWaveVolume(waveVolume - VOLUME_STEP)
            return true
        elseif data.key == "MUTE" then
            toggleWaveMute()
            return true
        end
        return false
    end
)
mediaKeyTap:start()

hs.alert.show("Wave volume keys active")
