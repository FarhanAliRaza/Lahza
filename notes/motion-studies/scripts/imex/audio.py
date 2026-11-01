import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
hop=256
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop)
tempo,beats=librosa.beat.beat_track(onset_envelope=oenv,sr=sr,hop_length=hop)
bt=librosa.frames_to_time(beats,sr=sr,hop_length=hop)
print("tempo",tempo)
print("beats",[round(float(b),3) for b in bt])
print("beat frames@30",[round(float(b)*30,1) for b in bt])
ons=librosa.onset.onset_detect(onset_envelope=oenv,sr=sr,hop_length=hop,backtrack=False)
ot=librosa.frames_to_time(ons,sr=sr,hop_length=hop)
st=oenv[ons]
print("onsets (t, frame30, strength):")
for t,s in zip(ot,st): print(f"  {t:6.3f} {t*30:6.1f} {s:5.2f}")
# tempogram estimate
ac=librosa.feature.tempo(onset_envelope=oenv,sr=sr,hop_length=hop,aggregate=None)
print("tempo median",np.median(ac))
# per-video-frame RMS and spectral centroid, band energies
fl=sr//30
S=np.abs(librosa.stft(y,n_fft=2048,hop_length=fl))
freqs=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=S[freqs<150].sum(0); mid=S[(freqs>=150)&(freqs<2000)].sum(0); hi=S[freqs>=4000].sum(0)
rms=librosa.feature.rms(y=y,frame_length=2048,hop_length=fl)[0]
np.savetxt(sys.argv[2],np.c_[np.arange(len(rms)),rms,low,mid,hi],fmt="%.5f")
