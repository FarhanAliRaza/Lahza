import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
hop=256
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop)
tt=librosa.times_like(oenv,sr=sr,hop_length=hop)
for a,b in ((0,4.1),(4.1,12.7),(12.7,25.3),(4.1,25.3),(0,29.9)):
    m=(tt>=a)&(tt<b)
    tg=librosa.feature.tempo(onset_envelope=oenv[m],sr=sr,hop_length=hop,max_tempo=200)
    # autocorr peaks
    e=oenv[m]-oenv[m].mean(); ac=np.correlate(e,e,'full')[len(e)-1:]
    lags=np.arange(len(ac))*hop/sr
    sel=(lags>0.25)&(lags<1.5)
    idx=np.argsort(ac[sel])[::-1][:4]
    print(f"window {a}-{b}s tempo {tg[0]:.1f}  acf top lags(s):",[round(float(lags[sel][i]),3) for i in idx], "-> bpm",[round(60/float(lags[sel][i]),1) for i in idx])
# band onsets with backtrack
S=np.abs(librosa.stft(y,n_fft=1024,hop_length=hop))
f=librosa.fft_frequencies(sr=sr,n_fft=1024)
bands={'sub<150':f<150,'mid150-2k':(f>=150)&(f<2000),'hi>3k':f>3000}
res={}
for name,mk in bands.items():
    env=librosa.onset.onset_strength(S=librosa.amplitude_to_db(S[mk]),sr=sr,hop_length=hop)
    on=librosa.onset.onset_detect(onset_envelope=env,sr=sr,hop_length=hop,backtrack=False,delta=0.25,wait=4)
    t=librosa.frames_to_time(on,sr=sr,hop_length=hop)
    s=env[on]/env.max()
    keep=s>0.25
    res[name]=list(zip(t[keep],s[keep]))
    print(name, ' '.join(f"{x:.3f}({v:.2f})" for x,v in zip(t[keep],s[keep])))
np.save(sys.argv[2],{k:np.array(v) for k,v in res.items()},allow_pickle=True)
