# BEARS → Bear Den rename checklist

Canonical naming:
- Product / brand: `Bear Den`
- Service: `Den`
- Technical namespace: prefer `den`
- Do not introduce `bear_den` as the default technical namespace

## Audit searches
- [ ] Search for `BEARS`
- [ ] Search for `Bears`
- [ ] Search for `bears`
- [ ] Search for `BEARS_`
- [ ] Search for `bears_`
- [ ] Search for `bears.`
- [ ] Search for `bears-`

## Classification rubric
For each occurrence, classify as one of:
- [ ] Branding → replace with `Bear Den`
- [ ] Service/runtime → replace with `Den`
- [ ] Technical namespace → replace with `den`
- [ ] Compatibility surface → alias and deprecate
- [ ] Historical reference → leave intentionally or annotate

## Low-risk branding updates
- [ ] README and top-level docs
- [ ] App/site titles and descriptions
- [ ] Product-facing UI copy
- [ ] Repo descriptions and metadata
- [ ] Onboarding, email, and social metadata

## Technical namespace migration
- [ ] Replace `BEARS_*` env vars with `DEN_*`
- [ ] Replace `bears.*` config keys with `den.*`
- [ ] Update technical docs to prefer `DEN_*` / `den.*`
- [ ] Add compatibility aliases for public/deployed surfaces where needed

## Verification
- [ ] Search again for `BEARS`, `Bears`, and `bears`
- [ ] Confirm remaining hits are intentional
- [ ] Verify docs/examples use `Bear Den` and `den`
- [ ] Verify compatibility behavior where promised
- [ ] Update tests and snapshots as needed

## Cleanup
- [ ] Remove deprecated aliases after migration window
- [ ] Remove obsolete BEARS branding assets
- [ ] Add guardrails to prevent reintroduction
