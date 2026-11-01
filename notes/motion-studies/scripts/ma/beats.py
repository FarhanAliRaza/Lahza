import sys, numpy as np, librosa
y,sr=librosa.load(sys.argv[1],sr=22050)
oenv=librosa.onset.onset_strength(y=y,sr=sr,hop_length=256)
tempo,beats=librosa.beat.beat_track(onset_envelope=oenv,sr=sr,hop_length=256,units='time')
print('tempo',tempo)
print('beats',' '.join(f'{b:.3f}' for b in beats))
on=librosa.onset.onset_detect(onset_envelope=oenv,sr=sr,hop_length=256,units='time',backtrack=False)
st=oenv[librosa.time_to_frames(on,sr=sr,hop_length=256)]
print('onsets (t, strength/max)')
mx=oenv.max()
for t,s in zip(on,st): print(f'{t:.3f} f{t*25:.1f} {s/mx:.2f}')
# tempogram autocorr tempo candidates
ac=librosa.autocorrelate(oenv,max_size=int(4*sr/256))
lags=np.arange(len(ac))*256/sr
idx=[i for i in range(1,len(ac)-1) if ac[i]>ac[i-1] and ac[i]>ac[i+1] and lags[i]>0.25]
idx=sorted(idx,key=lambda i:-ac[i])[:6]
print('ac peaks (lag s, bpm, rel)',[(round(lags[i],3),round(60/lags[i],1),round(ac[i]/ac[0],2)) for i in idx])
# RMS per video frame
hop=sr//25
rms=[float(np.sqrt(np.mean(y[i*hop:(i+1)*hop]**2))) for i in range(len(y)//hop)]
np.savetxt(sys.argv[2],np.array(rms))
# bass vs high band energy per frame for hit classification
S=np.abs(librosa.stft(y,n_fft=2048,hop_length=hop))
f=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=S[f<150].sum(0); high=S[f>4000].sum(0)
np.savetxt(sys.argv[3],np.vstack([low,high]).T)
