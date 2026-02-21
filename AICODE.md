# AI coding policy

## Purpose

This policy defines the requirements for using AI-assisted code generation
tools in the development of security-sensitive software. AI-generated code is
treated as untrusted third-party input and must undergo verification
appropriate to the sensitivity of the modified component.

Compliance is based on verification process completion, not developer
seniority, academic degree, or experience level.

## Component Classification

### Level 0 - Non-Security Components

Examples:

* Documentation
* UI layout
* Formatting
* Non-functional logging
* Tests unrelated to security behavior

Requirements:

* Standard peer review

### Level 1 - Functional Components

Examples:

* Business logic
* Data transformations
* Non-privileged services
* Internal helpers

Requirements:

* Peer review
* Static analysis checks must pass

### Level 2 - Boundary Components

Examples:

* Network protocol handling
* Parsers and deserialization
* IPC communication
* Configuration loaders
* Permission mapping
* External data handling

Requirements:

* Two independent reviewers
* Security checklist review
* Input validation verification
* Negative test coverage

### Level 3 - Security-Critical Components

Examples:

* Authentication and authorization
* TLS and cryptographic operations
* Key management
* Token generation or validation (JWT or similar)
* Privileged core modules
* Memory-unsafe FFI
* Access control enforcement

Requirements:

* Design review prior to implementation
* Manual code review by designated security maintainer
* Human-written explanation of algorithm and trust assumptions
* Test vectors or differential testing
* Fuzzing or adversarial test coverage
* AI-generated code may not be merged directly

## AI Usage Rules

* AI-assisted contributions must be declared in the commit message.
* Developers must fully understand and be able to explain submitted code.
* AI output must not be merged without required verification for its classification level.
* AI tools are treated as untrusted external contributors.
* Tests validating security behavior must be written without AI assistance.
* Any modification affecting Level 3 components requires explicit security approval.

## Approval Authority

Approval is granted by role, not by seniority:

* Peer Reviewer - Level 0-1
* Senior Reviewer - Level 2
* Security Maintainer - Level 3

## Responsibility

The merging reviewer is responsible for confirming that verification
requirements were completed. Use of AI tools does not transfer responsibility
from the human contributor or reviewer.

## Enforcement

* Non-compliant changes must not be merged.
* Violations require security review before release.

