import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
hop=256
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop)
tempo,beats=librosa.beat.beat_track(onset_envelope=oenv,sr=sr,hop_length=hop)
bt=librosa.frames_to_time(beats,sr=sr,hop_length=hop)
print("tempo",tempo)
print("beats",np.round(bt,3).tolist())
print("beat intervals",np.round(np.diff(bt),3).tolist())
on=librosa.onset.onset_detect(onset_envelope=oenv,sr=sr,hop_length=hop,backtrack=False)
ot=librosa.frames_to_time(on,sr=sr,hop_length=hop)
st=oenv[on]
mx=st.max()
print("onsets (t, frame30, strength):")
for t,s in zip(ot,st): print(f"{t:.3f} f{t*30:.1f} {s/mx:.2f}")
# tempogram estimate
tg=librosa.feature.tempo(onset_envelope=oenv,sr=sr,hop_length=hop,aggregate=None)
print("local tempo median",np.median(tg))
# RMS per video frame
rms=librosa.feature.rms(y=y,frame_length=1470,hop_length=735)[0]
db=20*np.log10(rms+1e-9)
print("RMS dB per 0.5s:",[round(float(np.mean(db[i:i+15])),1) for i in range(0,len(db),15)])
# spectral bands per 0.25s
S=np.abs(librosa.stft(y,n_fft=2048,hop_length=hop))
f=librosa.fft_frequencies(sr=sr,n_fft=2048)
bands=[(20,150),(150,1000),(1000,4000),(4000,11000)]
step=int(0.25*sr/hop)
for name,(a,b) in zip(["sub","low-mid","mid","hi"],bands):
    e=S[(f>=a)&(f<b)].sum(0)
    e=20*np.log10(e+1e-9)
    print(name,[round(float(e[i:i+step].mean()),0) for i in range(0,len(e),step)])
