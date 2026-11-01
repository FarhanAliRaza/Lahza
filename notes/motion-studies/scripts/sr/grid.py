import sys, numpy as np, librosa
y, sr = librosa.load(sys.argv[1], sr=22050)
hop=110  # 5ms
oenv = librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop)
t = np.arange(len(oenv))*hop/sr
def score(p, ph, a, b):
    m=(t>=a)&(t<b); g=np.arange(ph, b, p); g=g[g>=a]
    idx=np.clip(np.round(g*sr/hop).astype(int),0,len(oenv)-1)
    # take max within +-10ms
    s=sum(oenv[max(0,i-2):i+3].max() for i in idx)
    return s/len(idx)
for (a,b) in [(0,35),(0,10),(10,20),(20,29),(29,35)]:
    best=(0,0,0)
    for p in np.arange(0.26,0.31,0.0005):
        for ph in np.arange(0,p,0.005):
            s=score(p,ph,a,b)
            if s>best[0]: best=(s,p,ph)
    print(f"[{a},{b}) best 8th period {best[1]:.4f}s -> {60/best[1]/2:.2f} BPM (quarter), phase {best[2]:.3f}, score {best[0]:.2f}")
# RMS per 0.5s for structure
rms = librosa.feature.rms(y=y, frame_length=2048, hop_length=512)[0]
tt=np.arange(len(rms))*512/sr
for s0 in np.arange(0,35.5,1.0):
    m=(tt>=s0)&(tt<s0+1); print(f"{s0:4.0f}s rms {20*np.log10(rms[m].mean()+1e-9):6.1f} dB", end=" | " if int(s0)%4!=3 else "\n")
# spectral: low band energy per second
S_=np.abs(librosa.stft(y,n_fft=2048,hop_length=512)); f=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=S_[f<150].sum(0); hi=S_[f>4000].sum(0)
print()
for s0 in np.arange(0,35.5,1.0):
    m=(tt>=s0)&(tt<s0+1); print(f"{s0:4.0f}s low {20*np.log10(low[m].mean()+1e-9):5.1f} hi {20*np.log10(hi[m].mean()+1e-9):5.1f}", end=" | " if int(s0)%4!=3 else "\n")
