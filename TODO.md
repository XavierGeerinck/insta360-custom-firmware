

## GENERAL

- [ ] Where are all the AI Models
- [ ] What kind of improvements are quick wins?
- [ ] Can Claude solve the issue that I ma having?
- [ ] I need it open and be able to send to my own services (configurable) but compatible with existing Wave Controller
- [ ] estimate how long to rebuild it open source
    - [ ] (potentially in Rust?)
    - [ ] what's the MVP firmware? how can I revert back?
    - [ ] what's the thing with the NPU? why can't we use it better?



- Lower ec_aggr (from 1.5 to ~0.8)
- Increase agc_alpha (from 0.1 to ~0.3 for faster adaptation)
- Decrease AGC attack time (from 0.5 to ~0.1)
- Add a proper double-talk detector (DTD) that prevents the AEC from adapting during silence periods near a near-end speaker
- Tune the AMM to maintain a minimum baseline gain per channel rather than fully attenuating silent channels


 Quick summary of the fix you can try right now: Go to Settings → Audio Mode → Original → Omni. This switches from the "meeting" pipeline (which applies the RAFA neural noise mask + NNPOST AGC that cause the slow convergence) to the "live"
  pipeline which disables both (disable_NNMASK: 1, agc_gain_apply: 0). The live pipeline also has 5 dB more input gain and allows up to 15 dB of recovery gain vs 11.5 dB.