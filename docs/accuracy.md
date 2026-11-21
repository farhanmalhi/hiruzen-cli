# Measuring accuracy

`hiruzen evaluate` measures how good the suggestions are for you. It learns
from the older four fifths of your shell history, replays the newest fifth
word by word, and reports how often the word you really typed next was the
first suggestion, in the top three, or in the top eight. It needs at least
200 commands of history, prints totals only, and runs no commands.

Suggestions cannot always be right: where several words are valid, hiruzen
ranks them by the spec's priority and by what you have typed in the same
position before.

Two things decide whether the right word is suggested: whether it can be
known at all, and how well the knowable words are ordered. A new commit
message, a branch you are about to create, or a file hiruzen cannot see can
never be suggested, so the first limits every tool of this kind.

**Command names, on public research data.** On the first 5,000 commands of
each of the 50 users in Schonlau's command data set (38,413 test cases),
with two letters typed:

| Setting | Right command is first | In the top three | In the top eight |
| --- | --- | --- | --- |
| Ranked by how often and how recently you use it | 91.2% | 98.7% | 99.7% |
| Also using the command you ran just before (the default) | 96.8% | 99.5% | 99.7% |

Using the previous command is an idea from published work on predicting
UNIX commands (Davison and Hirsh, 1998). That data set holds command names
only, so it says nothing about arguments.

**Whole commands, on one personal history** (about 280 commands, a small
sample; live values such as branches and files are not available to the
test, so real use is higher):

| Case | Right word is first | In the top eight | First, when the word was offered at all |
| --- | --- | --- | --- |
| Command name, after two letters | 72% | 92% | 78% |
| Next word, after two letters | 50% | 55% | 91% |
| Next word, nothing typed | 31% | 49% | 58% |

In that test, of the next words that were not in the top eight after two
letters, half were paths, file names or versions and half were text never
typed before; one was an option a spec did not list. `hiruzen evaluate`
prints this breakdown for your own history.

Computing suggestions took a median of 0.07 ms, with 95% under 0.5 ms and
the slowest at 13 ms (the first use of a large spec). That excludes commands
a spec runs for live values, which take as long as the tool takes and never
block typing.

`hiruzen evaluate --file commands.txt` measures against any file of
commands, one per line and oldest first.
