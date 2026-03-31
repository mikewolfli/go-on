---
name: "breprs-completion-review"
description: "Reviews BrepRs completion plans and provides comprehensive, optimized implementation strategies. Invoke when user asks to review completion plans or needs comprehensive implementation guidance for BrepRs project."
---

# BrepRs Completion Review Skill

This skill provides comprehensive review and optimization strategies for BrepRs project completion plans. It analyzes the current state of the project, identifies gaps, and provides detailed implementation guidance.

## When to Use This Skill

Invoke this skill when:
- User asks to review COMPLETION_PLAN.md or similar project completion documents
- User needs comprehensive implementation strategies for BrepRs project
- User wants optimized solutions for missing functionality
- User requests complete, perfect, and comprehensive implementation plans

## Skill Capabilities

1. **Plan Analysis**: Deep analysis of completion plans, identifying priorities and dependencies
2. **Implementation Strategy**: Provides detailed, step-by-step implementation strategies
3. **Optimization Guidance**: Offers optimized solutions considering performance, maintainability, and best practices
4. **Dependency Mapping**: Identifies module dependencies and implementation order
5. **Testing Strategy**: Recommends comprehensive testing approaches
6. **Documentation Guidance**: Suggests documentation standards and practices

## Review Process

When reviewing a completion plan, this skill will:

1. **Analyze Current State**: Review the existing codebase structure and implementation
2. **Identify Critical Gaps**: Prioritize missing functionality based on project dependencies
3. **Provide Implementation Roadmap**: Create detailed implementation steps
4. **Optimize Architecture**: Suggest architectural improvements
5. **Ensure Completeness**: Verify all required functionality is covered
6. **Recommend Best Practices**: Suggest industry-standard patterns and practices

## Output Format

The skill provides structured output including:
- Executive summary of findings
- Priority-based implementation plan
- Detailed technical specifications
- Testing and validation strategies
- Documentation requirements
- Performance optimization suggestions

## Example Usage

When reviewing COMPLETION_PLAN.md, the skill will:
1. Analyze module completeness scores
2. Identify critical path dependencies
3. Provide specific implementation details for each module
4. Suggest testing strategies
5. Recommend documentation standards
6. Offer performance optimization tips

## Integration with Project Rules

This skill respects and follows the project's mandatory rules:
- No placeholder implementations
- Complete, production-ready code
- WASM compatibility
- Proper error handling
- Architecture compliance
- Cargo feature checks
- Self-validation before output