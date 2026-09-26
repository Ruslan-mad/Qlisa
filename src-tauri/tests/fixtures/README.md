# Test fixtures

`tiny_slice.mp3` is a synthetic test signal, not a third-party media asset. It
contains a one-second, 440 Hz sine wave at 48 kHz, encoded as stereo MP3 at
64 kbit/s. It was generated with FFmpeg and is 8,300 bytes.

```powershell
ffmpeg.exe -y -v error -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -ac 2 -codec:a libmp3lame -b:a 64k -write_xing 0 tiny_slice.mp3
```
