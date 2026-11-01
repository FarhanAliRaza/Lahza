import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
oenv=librosa.onset.onset_strength(y=y,sr=sr)
tempo,beats=librosa.beat.beat_track(onset_envelope=oenv,sr=sr)
bt=librosa.frames_to_time(beats,sr=sr)
print("tempo",tempo)
print("beats",np.round(bt,3).tolist())
on=librosa.onset.onset_detect(onset_envelope=oenv,sr=sr,units='frames')
ot=librosa.frames_to_time(on,sr=sr)
st=oenv[on]
print("onsets (t, strength, frame30):")
for t,s in zip(ot,st): print(f"{t:.3f}\t{s:.2f}\t{t*30:.1f}")
# tempogram alt estimates
ac=librosa.feature.tempo(onset_envelope=oenv,sr=sr,aggregate=None)
print("tempo median local",np.median(ac))
# RMS per video frame
hop=sr//30
rms=librosa.feature.rms(y=y,frame_length=2048,hop_length=735)[0]
np.savetxt(sys.argv[2],rms,fmt="%.4f")
# low/high band onset
S=np.abs(librosa.stft(y,n_fft=2048,hop_length=512))
f=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=S[f<150].sum(0); high=S[f>4000].sum(0)
for name,band in (("low",low),("high",high)):
    d=np.maximum(0,np.diff(np.log1p(band)))
    pk=librosa.util.peak_pick(d,pre_max=5,post_max=5,pre_avg=10,post_avg=10,delta=0.3*d.max(),wait=5)
    print(name,"peaks",[round(float(librosa.frames_to_time(p+1,sr=sr,hop_length=512)),3) for p in pk])
