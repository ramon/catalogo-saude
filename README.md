# Catálogo Saúde

O Catálogo Saúde reúne dados públicos de medicamentos e outros catálogos de saúde em arquivos JSON prontos para integração. Ele baixa as fontes, cruza os registros relacionados e normaliza os dados; não se conecta nem grava dados diretamente em um banco.

## Catálogos

- **Medicamentos:** combina dados abertos, consulta de produtos e preços da Anvisa.
- **Cosméticos e produtos de cannabis:** consulta de produtos da Anvisa.
- **Portaria 344:** listas de substâncias sujeitas a controle especial.
- **SIGTAP, TUSS e CID-10:** procedimentos, vínculos com a TUSS e códigos CID-10.

O catálogo DCB não faz parte desta coleta. Arquivos regulatórios já mantidos em `references/anvisa/` são usados localmente.

## Executar

É necessário ter o [mise](https://mise.jdx.dev/) instalado. O projeto fixa a versão do Rust em `mise.toml`; instale-a e execute uma coleta em um diretório próprio:

```sh
mise install
mise exec -- cargo run --release -- --output ./runs/minha-coleta
```

O comando exibe o andamento de cada fonte e etapa. Para escolher outro tamanho de lote, use `--batch-size` (o padrão é `1000`).

Também é possível baixar o executável da [versão v0.2.1](https://github.com/ramon/catalogo-saude/releases/tag/v0.2.1), atualmente publicada para Linux x86_64 com GNU libc. Depois de extrair o pacote:

```sh
./catalogo-saude-release-v0.2.1/catalogo-saude --output ./runs/minha-coleta
```

O executável recebe as mesmas opções da CLI, por exemplo `--output` e `--batch-size`. Mantenha a pasta `references/` incluída no pacote junto ao executável; ela permite executar a coleta fora do repositório. As referências do pacote têm prioridade sobre as da pasta atual.

## Arquivos gerados

Cada coleta cria um diretório com os lotes JSON por catálogo, os arquivos brutos usados e dois arquivos de controle:

- `manifest.json`: fontes e URLs, SHA-256, quantidade de registros e tempo de cada etapa.
- `download-control.json`: nome e data de cada download, caminho do arquivo e SHA-1 usado para validar sua integridade.
- `sources/`: cópias dos arquivos baixados, preservadas para consulta e reprocessamento.

Os lotes de medicamentos, cosméticos e cannabis são separados por situação regulatória:

```text
medicines/ativo/batch-000001.json
medicines/inativos/batch-000001.json
cosmetics/ativo/batch-000001.json
cosmetics/inativos/batch-000001.json
cannabis/ativo/batch-000001.json
cannabis/inativos/batch-000001.json
```

A pasta `ativo/` recebe medicamentos com situação `Ativo`, cosméticos com `S` e cannabis com `Válido`. A pasta `inativos/` recebe, respectivamente, `Inativo`, `N` e `Caduco/Cancelado`. Os valores de `regulatory_status` são preservados nos registros; situações desconhecidas interrompem a execução.

Cada pasta possui numeração própria, com até mil registros por lote por padrão. As duas pastas são criadas mesmo quando uma delas estiver vazia. Ao reprocessar, são removidos os lotes antigos dessas pastas e, após a gravação, os lotes do formato anterior diretamente na pasta do catálogo. Outros arquivos são preservados. Portaria 344 e SIGTAP continuam em `portaria-344/lists/` e `sigtap/`.

Guarde o diretório da coleta se quiser manter as fontes usadas. Para normalizar novamente usando esses downloads, informe o mesmo diretório e `--reuse-sources`:

```sh
mise exec -- cargo run --release -- --output ./runs/minha-coleta --reuse-sources
```

O loader valida o SHA-1 antes de reutilizar cada fonte. Se um arquivo preservado tiver sido alterado, o catálogo correspondente falha em vez de usar esse arquivo silenciosamente; os demais continuam.

Downloads interrompidos por falhas de conexão, corpo incompleto ou HTTP 408/429/500/502/503/504 são repetidos até quatro tentativas, com intervalos de 1, 2 e 4 segundos. Cada tentativa reinicia a transferência e descarta o arquivo parcial; somente downloads completos entram no controle. Erros permanentes, como HTTP 404 ou falta de permissão de escrita, não recebem essas tentativas.

Uma falha de download, normalização ou gravação de um catálogo não interrompe os demais. O `manifest.json` é gravado ao final e inclui `catalog_errors`, um objeto com os erros por catálogo, somente quando houver falhas. As contagens do catálogo que falhou são omitidas; lotes existentes ou parciais desse catálogo não devem ser considerados uma geração concluída. SIGTAP, TUSS e CID-10 são processados como uma etapa conjunta, pois compartilham fontes e cruzamentos.

A execução termina com código diferente de zero **depois de processar todos os catálogos**, se algum falhar. Para retomar aproveitando as fontes completas, execute novamente com o mesmo `--output` e `--reuse-sources`. A Portaria 344 é processada independentemente mesmo se medicamentos falhar; medicamentos precisam das listas para serem classificados.

## Listas da Portaria 344 nos medicamentos

Cada medicamento inclui `portaria_344_lists`, um array ordenado de códigos como `["A2"]` ou `["C1", "C5"]`. A classificação usa as mesmas listas da fonte baixada que geram os lotes em `portaria-344/lists/`.

Primeiro são consideradas as restrições regulatórias do tipo `P` e sua descrição: quando o tipo de receita identifica uma única lista, ela é usada diretamente. Se a receita não identificar uma lista ou corresponder a várias (como a notificação A), a busca usa as substâncias de todas as linhas de consulta do produto, com o princípio ativo dos dados abertos como alternativa quando estiverem ausentes.

A comparação ignora diferenças de caixa, acentos e pontuação, considera nomes alternativos da lista e aceita o nome da substância dentro de um composto, como `cloridrato de tramadol`, quando o adendo da lista inclui essas formas. Os limites entre palavras evitam confundir nomes como `metadona` e `levometadona`. Adendos de padrões analíticos não autorizam essa inclusão para medicamentos; nomes explicitamente listados e exceções identificáveis têm precedência. Regras condicionais de dose, concentração ou uso não são calculadas a partir do nome da substância.

Sem restrição do tipo `P` ou sem correspondência suficiente, o campo é `[]`; esse valor indica que a lista não foi determinada pelo cruzamento. Os demais campos dos medicamentos e os formatos dos arquivos de controle são preservados.

## Classes terapêuticas

O campo `therapeutic_classes` contém as descrições de `CLASSE_TERAPEUTICA` dos dados abertos de medicamentos da Anvisa, vinculadas por processo ou, na ausência de correspondência por processo, por registro. As classes das linhas vinculadas ao produto são reunidas sem duplicatas e ordenadas; o texto é preservado, sem dividir descrições por vírgulas ou inferir códigos ATC. Quando a fonte não informa a classe ou não há correspondência, o campo é `[]`.

## Tipos de receita do SNCR

O medicamento inclui `sncr_prescription_types`, um array ordenado com os tipos determinados: `NRA`, `NRB`, `NRB2`, `NRR`, `NRT`, `RCE` e `RET`. Restrições específicas do tipo `P` têm prioridade sobre o enquadramento geral da lista, preservando casos como A2 com RCE. Retenção informada sem vínculo com a Portaria 344 gera `RET`, não `RCE`.

O cruzamento distingue talidomida (`NRT`) dos demais C3 e exige evidência de uso sistêmico para C2 (`NRR`). Formas tópicas de C2/C5 não recebem esses tipos. As exceções nominais de B1, como fenobarbital, usam RCE. Quando a restrição não resolve um adendo dependente de dose ou formulação, o catálogo deixa o tipo indeterminado; não usa o volume da embalagem como dose. C3 fora de talidomida e listas de precursores/proibição não geram um dos sete tipos automaticamente.

`[]` significa que nenhum desses tipos foi determinado; não significa dispensa de prescrição. As listas, restrições e descrições originais continuam disponíveis para conferência. A fundamentação e as exceções estão em [pesquisa sobre o SNCR](docs/research/sncr-prescription-types.md).

## Descrições de apresentações

As descrições de apresentações usam o [Vocabulário Controlado da Anvisa](https://www.gov.br/anvisa/pt-br/centraisdeconteudo/publicacoes/medicamentos/publicacoes-sobre-medicamentos/vocabulario-controlado.pdf/@@display-file/file) para expandir formas farmacêuticas, vias de administração e embalagens. Siglas sem correspondência nesse vocabulário permanecem como vieram da fonte.

Cada apresentação também inclui `concentration` e `physical_form`. A concentração mantém o texto e as unidades da fonte, inclusive associações (`600 MG + 200 UI`), razões (`50 MCG/ML`), percentuais e decimais com vírgula. A forma é extraída dos códigos conhecidos no início da apresentação, após a concentração, incluindo a via quando reconhecida; embalagens e acessórios são excluídos. Exemplo: `50 MCG/ML SOL INJ CX 5 AMP X 10 ML` gera `concentration: "50 MCG/ML"` e `physical_form: "Solução Injetável"`.

Campos não identificados recebem `null`. Códigos sem correspondência oficial, como `COMP`, não ganham expansão presumida. `description`, `expanded_description` e o campo `physical_forms` do produto são preservados. A concentração não é convertida em dose, nem associada por posição a uma substância.

## Memória e processamento de cosméticos

A coleta libera as fontes que já não são necessárias. Downloads e validação de hashes leem e gravam os arquivos em blocos; a normalização de cosméticos lê e decodifica uma linha por vez. As fontes brutas continuam preservadas em `sources/`, com os mesmos campos de controle e validação SHA-1 para reutilização.

Os cosméticos usam 32 partições CSV temporárias, agrupadas pelo número de processo. Cada partição é deduplicada separadamente, mantendo a validação de registros conflitantes e a precedência da situação inativa. Os registros em memória usam campos compactos e compartilham nomes de fabricantes. Os lotes finais continuam separados em `ativo/` e `inativos/`, com numeração contínua por pasta. A ordem dos cosméticos nos lotes pode variar entre execuções.

As partições são criadas no diretório da coleta e removidas ao terminar, inclusive nos erros tratados. Esse processamento usa espaço temporário em disco; uma interrupção abrupta do processo pode deixar a pasta `.cosmetics-partitions-*`, sem alterar as fontes brutas.

### Medir alterações

O script de benchmark requer Python 3 e Linux com `/proc`. Compile antes da medição para excluir o tempo de compilação:

```sh
mise exec -- cargo build --release --locked
python3 scripts/benchmark_memory.py \
  --binary target/release/catalogo-saude \
  --source-run runs/minha-coleta \
  --output runs/benchmark-memoria \
  --label referencia
```

Após uma alteração, recompile e repita com outro nome, comparando com a referência:

```sh
python3 scripts/benchmark_memory.py \
  --binary target/release/catalogo-saude \
  --source-run runs/minha-coleta \
  --output runs/benchmark-memoria \
  --label alteracao \
  --baseline runs/benchmark-memoria/referencia.json
```

O script usa as mesmas fontes preservadas e lotes de 1.000 registros. Registra tempo externo, pico RSS do processo e amostras de memória por etapa, além de arquivar o binário e os logs. A comparação verifica hashes das fontes, contagens e uma assinatura SHA-256 do conteúdo JSON de todos os registros ordenados por assinatura; diferenças apenas na ordem dos lotes não afetam a verificação. A cópia preparatória das fontes e a verificação dos JSONs ficam fora do tempo medido. Os dados finais de cada medição ficam em `work/`, que é reprocessado a cada versão.

## Fontes

- [Dados abertos de medicamentos da Anvisa](https://dados.anvisa.gov.br/dados/DADOS_ABERTOS_MEDICAMENTOS.csv)
- [Consultas de produtos da Anvisa](https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/)
- [Portaria SVS/MS nº 344/1998](https://anvisalegis.datalegis.net/)
- [Tabelas SIGTAP](https://github.com/RenatoKR/SIGTAP)
- [Tabelas TISS da ANS](https://www.gov.br/ans/pt-br/assuntos/prestadores/padrao-para-troca-de-informacao-de-saude-suplementar-tiss)

## Licença

Este projeto está disponível sob a licença [MIT](LICENSE).
