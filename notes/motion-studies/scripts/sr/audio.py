import sys, numpy as np, librosa
y, sr = librosa.load(sys.argv[1], sr=22050)
hop=441  # 20ms -> exactly half a video frame
oenv = librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop)
tempo, beats = librosa.beat.beat_track(onset_envelope=oenv, sr=sr, hop_length=hop, units='time', tightness=200)
print("tempo", tempo)
print("beats", " ".join(f"{b:.2f}" for b in beats))
d=np.diff(beats); print("beat interval median", np.median(d), "mean", d.mean())
tg = librosa.feature.tempogram(onset_envelope=oenv, sr=sr, hop_length=hop)
ac = np.mean(tg,axis=1); bpms = librosa.tempo_frequencies(tg.shape[0], sr=sr, hop_length=hop)
idx=np.argsort(ac[1:])[::-1][:8]+1
print("tempogram peaks", [(round(bpms[i],1), round(ac[i],3)) for i in idx])
on = librosa.onset.onset_detect(onset_envelope=oenv, sr=sr, hop_length=hop, units='time', backtrack=False)
st = oenv[librosa.time_to_frames(on, sr=sr, hop_length=hop)]
print("onsets", " ".join(f"{t:.2f}:{s:.1f}" for t,s in zip(on,st)))
# strong onsets
thr=np.percentile(st,70)
print("strong", " ".join(f"{t:.2f}" for t,s in zip(on,st) if s>=thr))
# RMS per video frame
rms = librosa.feature.rms(y=y, frame_length=1764, hop_length=882)[0]
np.savetxt(sys.argv[2], rms, fmt="%.4f")
# percussive/low-band onsets for kick
S_=np.abs(librosa.stft(y,n_fft=2048,hop_length=hop)); f=librosa.fft_frequencies(sr=sr,n_fft=2048)
low=S_[f<150].sum(0); lowd=np.maximum(0,np.diff(np.log1p(low)))
pk=librosa.util.peak_pick(lowd,pre_max=5,post_max=5,pre_avg=10,post_avg=10,delta=0.15,wait=8)
print("kicks", " ".join(f"{p*hop/sr:.2f}" for p in pk))
hi=S_[f>5000].sum(0); hid=np.maximum(0,np.diff(np.log1p(hi)))
pk=librosa.util.peak_pick(hid,pre_max=5,post_max=5,pre_avg=10,post_avg=10,delta=0.3,wait=8)
print("hihat/clicks", " ".join(f"{p*hop/sr:.2f}" for p in pk))
