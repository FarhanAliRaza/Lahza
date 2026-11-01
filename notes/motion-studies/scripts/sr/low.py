import sys, numpy as np, librosa
y, sr = librosa.load(sys.argv[1], sr=22050)
S_=np.abs(librosa.stft(y,n_fft=2048,hop_length=441)); f=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=20*np.log10(S_[f<120].sum(0)+1e-6); full=20*np.log10(S_.sum(0)+1e-6)
t=np.arange(len(low))*441/sr
def show(a,b):
    for k in np.where((t>=a)&(t<b))[0][::2]: print(f"{t[k]:.2f}:{low[k]:.0f}/{full[k]:.0f}",end=" ")
    print()
for a,b in [(0.4,1.4),(9.6,11.2),(28.4,30.2),(30.2,31.2),(32.6,33.6),(33.6,35.1)]: show(a,b)
