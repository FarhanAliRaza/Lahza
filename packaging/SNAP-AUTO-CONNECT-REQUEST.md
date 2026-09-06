# Request auto-connection of camera and audio-record for lahza

Submission category: Store requests → Privileged interfaces
https://forum.snapcraft.io/c/store-requests/privileged-interfaces/27

Status: Prepared; not submitted.

---

Hello Snap Store reviewers,

I maintain and publish **Lahza**, an open-source Linux screenshot and screen-recording studio. I would like to request automatic connection of the `camera` and `audio-record` interfaces for the `lahza` snap.

- **Snap name:** `lahza`
- **Snap ID:** `CHvbDegXSLSjo8j3KIpgwIVdvmNp3mfR`
- **Publisher:** `farhanaliraza`
- **Store:** https://snapcraft.io/lahza
- **Upstream:** https://github.com/FarhanAliRaza/Lahza
- **Snapcraft configuration:** https://github.com/FarhanAliRaza/Lahza/blob/master/snap/snapcraft.yaml
- **Upstream relationship:** Published by the upstream maintainer.
- **License:** MIT
- **Confinement/base:** Strict confinement, core24.

## Requested interfaces and rationale

### camera — auto-connection

Lahza supports a webcam preview and webcam capture alongside screen recordings. It enumerates camera devices with GStreamer and uses V4L2 for webcam capture. Without this connection, a fresh installation cannot discover or open the webcam, even when the user explicitly enables Camera in the recording launcher.

Camera capture is an advertised, user-selected feature of the recording application. Automatic connection would let that control work as users expect immediately after installation.

### audio-record — auto-connection

Lahza records microphone narration and optional system audio alongside screen recordings. Its recording pipeline uses GStreamer’s PulseAudio source, including on systems running PipeWire’s PulseAudio compatibility service. Without `audio-record`, users cannot use these recording features normally and can see empty microphone selectors.

Recording narration and system sound is central to the application’s screen-recording purpose. Users currently have to discover and manually connect this interface before using those features.

## User control and scope

Camera, Microphone, and System audio are separate, visible controls and are off by default in a fresh launcher. Automatic interface connection would make the devices accessible to the application; it would not enable those controls or start a recording. Enabling Camera starts its visible preview, and recording starts only after the user requests it. The snap declares no background capture daemon.

Both interfaces are already declared in the snap configuration. This request is limited to their automatic connection; Lahza will retain strict confinement.

Users have reported that the installed snap cannot find their webcam or microphones. We confirmed that both interfaces were disconnected. We are also adding clearer in-app permission guidance and retry handling as a fallback, but automatic connection would remove this installation obstacle for the advertised recording features.

Thank you for reviewing this request.
