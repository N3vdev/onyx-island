# Generates the P0 ASR smoke-test WAVs with the built-in Windows SAPI voice.
# The audio never leaves the machine. Re-run from the repo root:
#   powershell -ExecutionPolicy Bypass -File tools/generate-test-audio.ps1
$ErrorActionPreference = "Stop"

$out = Join-Path $PSScriptRoot "..\assets\audio\test"
New-Item -ItemType Directory -Force -Path $out | Out-Null

$fixtures = @(
    @{
        name = "short-5s"
        text = "The quick brown fox jumps over the lazy dog near the river bank."
    },
    @{
        name = "medium-15s"
        text = "Local speech recognition keeps every word on this computer. No audio leaves the device, and the little island above the taskbar shows a live waveform while I speak. When I release the keys, the text appears wherever my cursor is."
    },
    @{
        name = "long-60s"
        text = "Dorothy lived in the midst of the great Kansas prairies, with Uncle Henry, who was a farmer, and Aunt Em, who was the farmer's wife. Their house was small, for the lumber to build it had to be carried by wagon many miles. There were four walls, a floor and a roof, which made one room; and this room contained a rusty looking cookstove, a cupboard for the dishes, a table, three or four chairs, and the beds. Uncle Henry and Aunt Em had a big bed in one corner, and Dorothy a little bed in another. There was no garret at all, and no cellar except a small hole dug in the ground, called a cyclone cellar, where the family could go in case one of those great whirlwinds arose, mighty enough to crush any building in its path. It was reached by a trap door in the middle of the floor, from which a ladder led down into the small, dark hole."
    }
)

$manifest = @()
foreach ($fixture in $fixtures) {
    $path = Join-Path $out ($fixture.name + ".wav")
    $voice = New-Object -ComObject SAPI.SpVoice
    $stream = New-Object -ComObject SAPI.SpFileStream
    $format = New-Object -ComObject SAPI.SpAudioFormat
    # SAPI.SpeechAudioFormatType: 18 = 16 kHz, 16-bit, mono.
    $format.Type = 18
    $stream.Format = $format
    $stream.Open($path, 3, $false)
    $voice.AudioOutputStream = $stream
    $voice.Rate = 0
    $voice.Volume = 100
    $voice.Speak($fixture.text) | Out-Null
    $stream.Close()

    $duration = python -c "import wave; w=wave.open(r'$path'); print(round(w.getnframes()/w.getframerate(), 2))"
    $manifest += [ordered]@{
        file = $fixture.name + ".wav"
        text = $fixture.text
        seconds = [double]$duration
    }
    Write-Host ("{0}: {1}s" -f $fixture.name, $duration)
}

$manifestJson = $manifest | ConvertTo-Json -Depth 4
[System.IO.File]::WriteAllText((Join-Path $out "manifest.json"), $manifestJson, (New-Object System.Text.UTF8Encoding($false)))
