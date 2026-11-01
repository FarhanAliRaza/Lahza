import sys, librosa, numpy as np
y,sr=librosa.load(sys.argv[1],sr=22050,mono=True)
H,P=librosa.effects.hpss(y,margin=3.0)
oe=librosa.onset.onset_strength(y=P,sr=sr)
tg=librosa.feature.tempogram(onset_envelope=oe,sr=sr)
ac=tg.mean(1); bpms=librosa.tempo_frequencies(tg.shape[0],sr=sr)
order=np.argsort(ac)[::-1]
print("top tempogram bpms:",[ (round(bpms[i],1),round(ac[i],3)) for i in order if 60<bpms[i]<200][:8])
t,b=librosa.beat.beat_track(onset_envelope=oe,sr=sr); bt=librosa.frames_to_time(b,sr=sr); iv=np.diff(bt)
print("perc tempo",t,"interval mean %.3f std %.3f"%(iv.mean(),iv.std()))
# low-band energy (<150Hz) per 0.5s to spot kick drum / music bed
S=np.abs(librosa.stft(y,n_fft=2048,hop_length=512)); f=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=S[f<120].sum(0); hop=int(0.5*sr/512)
print("low-band/0.5s",[round(float(low[i:i+hop].mean()),1) for i in range(0,len(low),hop)])
o=librosa.onset.onset_detect(onset_envelope=oe,sr=sr,units='time')
print("perc onsets",np.round(o,2).tolist())
