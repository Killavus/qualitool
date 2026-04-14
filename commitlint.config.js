module.exports = {
  extends: ['@commitlint/config-conventional'],
  plugins: [
    {
      rules: {
        'scope-linear-id': ({ scope }) => {
          if (!scope) return [true, ''];
          return [
            /^[A-Z]+-\d+$/.test(scope),
            'scope must be a Linear issue ID (e.g. QUA-123)',
          ];
        },
      },
    },
  ],
  rules: {
    'scope-empty': [2, 'never'],
    'scope-case': [0],
    'scope-linear-id': [2, 'always'],
    'type-enum': [
      2,
      'always',
      [
        'feat',
        'fix',
        'bugfix',
        'docs',
        'style',
        'refactor',
        'perf',
        'test',
        'build',
        'ci',
        'chore',
        'revert',
      ],
    ],
    'trailer-exists': [2, 'always', 'Project'],
  },
};
