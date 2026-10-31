# Onyx website

This static website introduces the native Onyx dynamic island for Windows.
Preserve the existing dark landing page, Geist typography, chapters, screenshots,
intro, and product claims outside the requested player correction.

The current request is to repair the live music island, use smooth 30-second
music clips, replace Strangers artwork with the exact Spotify release at
https://open.spotify.com/track/5mjYQaktjmjcMKcUIcqz4s, and match the app's design.
The user explicitly confirmed official 30-second preview excerpts are acceptable;
the clips do not promise the opening of the full song.

The native music renderer and shipped app captures are the player authority:
220 x 36 compact, 108 x 32 condensed after five seconds of playback, and 496 x 120
expanded; 4-unit art inset, Inter typography, native squircle geometry, fixed
playback control positions, and source/volume chips. The website adds the native
one-pixel boundary around these dimensions. Mobile scales the same geometry to
fit. Existing app code and behavior are outside this website task.

Keep the current five-song playlist. Stream official iTunes catalog previews
directly, limit each to 30 seconds, prewarm only the next clip, and retain pause
intent on skips. No music account, hidden playback iframe, backend proxy, or
audio download is required. Cover/title links retain the Spotify song identity;
the source chip and preview attribution link to the actual Apple Music catalog
entry. Provide pause, previous/next, seek, volume, mute, keyboard/touch controls,
and recoverable loading/error states. Respect reduced motion and avoid painting
the audio clock while paused or the document is hidden.
