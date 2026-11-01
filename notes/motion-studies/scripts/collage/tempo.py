import sys, librosa, numpy as np
y,sr=librosa.load(sys.argv[1],sr=22050,mono=True)
tempo,beats=librosa.beat.beat_track(y=y,sr=sr)
print("tempo",tempo, "nbeats",len(beats))
bt=librosa.frames_to_time(beats,sr=sr); print("beats",np.round(bt[:40],2).tolist())
# harmonic/percussive energy split
H,P=librosa.effects.hpss(y)
print("rms total %.4f harm %.4f perc %.4f"%(np.sqrt((y**2).mean()),np.sqrt((H**2).mean()),np.sqrt((P**2).mean())))
# energy during pauses: rms per 0.5s
r=librosa.feature.rms(y=y,frame_length=11025,hop_length=11025)[0]
print("rms/0.5s", np.round(r,3).tolist())
