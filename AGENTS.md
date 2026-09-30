# Instruções para agentes

## Escopo e regras de dados

- Este projeto coleta fontes públicas e gera lotes JSON para integração posterior. Não adiciona gravação em banco de dados.
- Preserve os formatos dos lotes e dos arquivos de controle existentes; confira o fluxo em `src/main.rs`, `src/downloads.rs`, `src/anvisa.rs`, `src/sigtap.rs` e `src/output.rs` antes de alterá-lo.
- Mantenha as referências regulatórias existentes em `references/anvisa/`; elas foram copiadas do `_dumps/anvisa` do projeto consumidor.
- Expanda descrições somente com códigos e componentes respaldados pelo [Vocabulário Controlado da Anvisa](https://www.gov.br/anvisa/pt-br/centraisdeconteudo/publicacoes/medicamentos/publicacoes-sobre-medicamentos/vocabulario-controlado.pdf/@@display-file/file), registrados em `references/presentation_abbreviations.json`. Preserve códigos sem correspondência oficial como vieram da fonte; não infira expansões.
- Cada execução conserva as fontes brutas em `runs/<execução>/sources/` e registra os downloads em `download-control.json`. Preserve esses arquivos quando não fizerem parte explícita da tarefa; `--reuse-sources` depende deles e valida o SHA-1.

## Alterações e verificações

- Use a versão do Rust definida em `mise.toml` e execute ferramentas Rust via mise.
- Para mudanças no código, execute `mise exec -- cargo fmt --check` e `mise exec -- cargo check --locked`. Execute `mise exec -- cargo test --locked` quando houver testes relevantes ou quando a tarefa solicitar testes.
- Escreva a documentação humana em português brasileiro; use inglês para código, identificadores e testes.
- Mantenha commits no padrão Conventional Commits. Atualize `Cargo.toml` e `Cargo.lock` juntos ao alterar a versão. Releases usam tags `vMAJOR.MINOR.PATCH`; identifique arquitetura e sistema operacional nos binários publicados e inclua checksum.
- Antes de concluir, revise `git diff --check` e preserve alterações não relacionadas.
