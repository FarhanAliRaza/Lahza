import sys, numpy as np, librosa
y, sr = librosa.load(sys.argv[1], sr=22050)
hop=110
oenv = librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop)
on = librosa.onset.onset_detect(onset_envelope=oenv, sr=sr, hop_length=hop, units='time')
st = np.array([oenv[max(0,i-3):i+4].max() for i in librosa.time_to_frames(on,sr=sr,hop_length=hop)])
st=st/np.percentile(st,95)
np.savetxt(sys.argv[3], np.c_[on,st], fmt="%.3f")
cuts=[float(x) for x in sys.argv[2].split(',')]
P=0.2855; ph=0.150
for c in cuts:
    m=st>0.45
    o=on[m]; s=st[m]; i=np.argmin(abs(o-c))
    g=ph+round((c-ph)/P)*P
    print(f"cut {c:6.2f}  nearest strong onset {o[i]:6.3f} (str {s[i]:.2f}) off {(c-o[i])*25:+5.1f}f | 8th-grid {g:6.3f} off {(c-g)*25:+5.1f}f  8th#{round((c-ph)/P)}")
