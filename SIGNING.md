# Trusted Windows code signing

Current status: **pending enrollment**. No trusted certificate or signing account was available when this project was created. Nothing is self-signed, and no unsupported trust claims are made. Version 0.1.0 is publicly available as an explicitly unsigned release. Trusted signing is required before the default automated workflow publishes future releases; the maintainer can explicitly publish an unsigned build.

## Free open-source route: SignPath Foundation

Apply through [SignPath Foundation](https://signpath.org/apply). Acceptance requires an existing, maintained open-source release and verifiable reputation, as well as MFA, declared author/reviewer/approver roles and compliance with their [terms](https://signpath.org/terms.html). Approval is discretionary; a new project is not guaranteed immediate acceptance. Identity, account creation and approval must be completed by the maintainer. This repository does not submit an application or accept legal terms on the maintainer's behalf.

After acceptance:

1. Connect the GitHub repository to SignPath, install its required GitHub integration, and configure the project/trusted build policy for the GitHub-hosted workflow.
2. Create an artifact configuration selecting the EXE inside the uploaded `windows-unsigned` artifact, and a **production** signing policy. Declare the actual authors, reviewers and signing approvers in the README.
3. Set repository secret `SIGNPATH_API_TOKEN` and repository variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`, `SIGNPATH_SIGNING_POLICY_SLUG`, `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`.
4. Run **Windows build and release** manually for tag `v0.1.0`. Approve its signing request in SignPath. The action waits up to one hour, downloads the signed EXE, validates Authenticode, and publishes/replaces the draft release.

The action uses uploaded GitHub artifact IDs and GitHub-hosted builds for provenance. See [SignPath's GitHub integration](https://docs.signpath.io/trusted-build-systems/github). Do not enable a test/self-signed policy for public releases: Windows signature verification must report `Valid`.

## Existing trusted certificate or signing service

Use the CA's supported hardware-token or managed signing tool. The code-signing certificate must chain to a Windows-trusted public CA and support Authenticode; a website HTTPS certificate does not qualify. Never put private keys or signing secrets in this repository.

With an accessible trusted certificate in the Windows user store and the Windows SDK's SignTool:

```powershell
signtool sign /sha1 YOUR_CERTIFICATE_THUMBPRINT /fd SHA256 /tr YOUR_CA_RFC3161_TIMESTAMP_URL /td SHA256 wardogs-earplugs.exe
signtool verify /pa /all /v wardogs-earplugs.exe
Get-AuthenticodeSignature .\wardogs-earplugs.exe
```

After a `Valid` result, package that signed EXE, regenerate SHA-256 checksums, replace the draft assets and publish. Hashes must cover the signed file, and the file must not be modified after signing.

Microsoft's [Artifact Signing eligibility](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart) currently covers organizations in the EU, USA, Canada and UK, and individual developers only in the USA/Canada. A Spain-based individual would need another provider, an eligible organization or Foundation acceptance. Paid services require the maintainer's account, identity validation and billing authorization.

No domain or changes to `danielgrao.me` are needed. A domain can link to the project but cannot provide an Authenticode identity by itself.

Even trusted signatures cannot guarantee no antivirus/SmartScreen warning. [Microsoft explains the reputation behavior](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation).
