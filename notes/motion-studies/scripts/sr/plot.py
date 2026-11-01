import sys, numpy as np, librosa, matplotlib; matplotlib.use('Agg'); import matplotlib.pyplot as plt
y, sr = librosa.load(sys.argv[1], sr=22050); hop=110
cuts=[float(x) for x in sys.argv[2].split(',')]
S_=np.abs(librosa.stft(y,n_fft=1024,hop_length=hop)); f=librosa.fft_frequencies(sr=sr,n_fft=1024)
def band(lo,hi):
    e=np.log1p(S_[(f>=lo)&(f<hi)].sum(0)); d=np.maximum(0,np.diff(e,prepend=e[0])); return d/np.percentile(d,99.5)
bands=[band(20,150),band(150,2000),band(2000,11000)]
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=hop); oenv/=np.percentile(oenv,99.5)
t=np.arange(len(oenv))*hop/sr
fig,axs=plt.subplots(5,1,figsize=(22,13))
for r,ax in enumerate(axs):
    a,b=r*7,(r+1)*7; m=(t>=a)&(t<b)
    ax.plot(t[m],oenv[m],'k',lw=.8,label='onset')
    for bd,c,n in zip(bands,['tab:blue','tab:green','tab:orange'],['low','mid','high']):
        ax.plot(t[m],-bd[m[:len(bd)]]*0.6-0.1*(['low','mid','high'].index(n)),c=c,lw=.7,label=n)
    for k in range(200):
        g=0.150+k*0.2855
        if a<=g<b: ax.axvline(g,color='0.75' if k%2 else '0.45',lw=.6,ls='--')
    for c in cuts:
        if a<=c<b: ax.axvline(c,color='red',lw=1.6)
    ax.set_xlim(a,b); ax.set_ylim(-0.9,1.1); ax.set_xticks(np.arange(a,b+0.01,0.2)); ax.tick_params(labelsize=7)
axs[0].legend(loc='upper right',fontsize=8)
plt.tight_layout(); plt.savefig(sys.argv[3],dpi=80)
