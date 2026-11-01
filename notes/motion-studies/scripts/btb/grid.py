import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
b=np.array([0.221, 0.708, 1.161, 1.602, 2.055, 2.508, 2.961, 3.402, 3.855, 4.307, 4.76, 5.213, 5.654, 6.107, 6.56, 7.012, 7.465, 7.906, 8.359, 8.812, 9.265, 9.706, 10.159, 10.6, 11.053, 11.494, 11.958, 12.411, 12.852, 13.305, 13.746, 14.199, 14.652, 15.105, 15.557, 16.01, 16.463])
k=np.arange(len(b)); p,t0=np.polyfit(k,b,1); print("period",p,"bpm",60/p,"t0",t0,"resid ms",np.round((b-(t0+k*p))*1000).astype(int).tolist())
# kick energy (<120Hz) and snare-ish (1-5k) envelopes at hop 128
hop=128
S=np.abs(librosa.stft(y,n_fft=2048,hop_length=hop)); f=librosa.fft_frequencies(sr=sr,n_fft=2048)
def env(a,c):
    e=S[(f>=a)&(f<c)].sum(0); d=np.diff(np.log(e+1e-6),prepend=0); return np.maximum(d,0)
kick=env(30,120); snr=env(1500,6000); hat=env(7000,11000)
tt=librosa.frames_to_time(np.arange(S.shape[1]),sr=sr,hop_length=hop)
# evaluate at subdivision grid: each beat, 3 triplets and 4 sixteenths
def at(e,t,w=0.03):
    m=(tt>t-w)&(tt<t+w); return e[m].max() if m.any() else 0
print("beat#  t   frame  kick  snare  hat")
for i in range(40):
    t=t0+i*p
    if t>17.2: break
    print(i, f"{t:.3f} f{t*30:.1f}", round(at(kick,t),2), round(at(snr,t),2), round(at(hat,t),2))
# subdivision test: mean onset at fractional positions
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop)
for frac in [0,1/4,1/3,1/2,2/3,3/4]:
    v=[at(oenv,t0+i*p+frac*p,0.015) for i in range(36)]
    print("frac",round(frac,3),round(float(np.mean(v)),2))
