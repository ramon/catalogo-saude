# Histórico de alterações

As mudanças relevantes do Catálogo Saúde estão registradas neste arquivo.
O formato segue [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/),
e as versões seguem [Semantic Versioning](https://semver.org/lang/pt-BR/).

## [Não lançado]

## [0.2.0] - 2026-10-02

### Adicionado

- Listas da Portaria 344 e tipos de receita do SNCR nos medicamentos, com
  distinção de exceções por substância e via de uso.
- Classes terapêuticas informadas nos dados abertos da Anvisa.
- Concentração e forma física em campos próprios das apresentações,
  preservando as descrições originais.
- Script e relatórios de medição de memória e cobertura dos dados.
- Referências regulatórias no pacote distribuído, carregadas junto ao executável.

### Alterado

- Lotes de medicamentos, cannabis e cosméticos separados em `ativo/` e
  `inativos/`, com numeração independente. Integrações que leem os caminhos
  anteriores precisam ser atualizadas.
- Cosméticos processados em partições temporárias, com deduplicação incremental,
  e downloads/validação de fontes em blocos. O pico médio de memória caiu de
  1.814,6 MiB para 582,6 MiB nas medições da otimização; o enriquecimento posterior
  dos medicamentos foi validado separadamente.

## [0.1.0] - 2026-09-30

### Adicionado

- Coleta e normalização de medicamentos, cosméticos, produtos de cannabis,
  Portaria 344 e catálogos SIGTAP, TUSS e CID-10 em lotes JSON.
- Preservação das fontes baixadas e registro de metadados, datas e hashes para
  auditoria e reutilização.
- Expansão de descrições de apresentações com base no Vocabulário Controlado da
  Anvisa.
- Binário para Linux x86_64 com GNU libc.

[Não lançado]: https://github.com/ramon/catalogo-saude/compare/v0.2.0...HEAD
[0.1.0]: https://github.com/ramon/catalogo-saude/releases/tag/v0.1.0

[0.2.0]: https://github.com/ramon/catalogo-saude/compare/v0.1.0...v0.2.0
