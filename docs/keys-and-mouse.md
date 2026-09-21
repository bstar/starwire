# Keys and the mouse

Every key STAR/WIRE knows, in the order the `?` overlay prints them. This
file is generated from the table in `src/ui/keymap.rs`, and a test fails if
the two disagree.

A key reaches its action through six layers, tried in order: an open overlay
takes every key while it is up; the `/` filter's text entry takes typing
next, because a letter typed into it is a letter, not a command; an `o`
waiting for a link number comes next; a `g` waiting for its second key comes
after that; the focused module's own bindings are offered the key, so a
binding under a module heading works while that module has focus; and the
global table catches whatever nothing above wanted, which is what makes it
work from everywhere. While the filter has focus, every `alt+…` falls
through it and so does `?`, so help and the appearance keys stay reachable
mid-search; `esc` and `enter` are always the way out.

`/` narrows the list under the keyboard, on every keystroke and without a
query: the feed list in the sources, the headlines in the entries, and the two
are independent of each other. In the reader there is no list to narrow —
finding text inside an article is a different thing — so `/` there moves to
the entries and filters those. `enter` keeps what it has narrowed to and
hands the keys back; `esc` clears the filter, whether the field is still open
or was put away with `enter`, and the status row says so in both states.

In the reader `o` waits: `o` again opens the article itself in the browser,
and a digit opens the link the article numbers with it. The number closes as
soon as it cannot grow — `o1` in an article with three links opens link
one at once, and in an article with twelve links it waits to see whether a `2`
follows.

## navigation

_everywhere_

| key | what it does |
|---|---|
| `tab`          | next module |
| `shift+tab`    | previous module |
| `alt+1`        | the sources |
| `alt+2`        | the entries |
| `alt+3`        | the reader |
| `up/k`         | up one |
| `down/j`       | down one |
| `shift+up/K`   | up ten |
| `shift+down/J` | down ten |
| `pgup`         | page up |
| `pgdn`         | page down |
| `home/gg`      | to the top |
| `end/G`        | to the bottom |
| `enter`        | open it |
| `esc`          | cancel |
| `l/right`      | drill in |
| `h/left/bs`    | back one level |
| `alt+up`       | jump to the parent |
| `alt+down`     | jump back down |
| `alt+f`        | search everything |
| `/`            | filter this list |

## sources

_in the sources_

| key | what it does |
|---|---|
| `r`            | refresh this source |
| `d/delete`     | remove the feed |
| `A`            | mark source read |

## entries

_in the entries_

| key | what it does |
|---|---|
| `m`            | read, unread |
| `s`            | star, unstar |
| `o`            | in the browser |
| `y`            | copy the link |
| `v`            | play the video |
| `n`            | next unread |
| `p`            | previous unread |
| `u`            | unread only |
| `A`            | mark all read |

## reader

_in the reader_

| key | what it does |
|---|---|
| `space`        | page down |
| `b`            | page up |
| `n`            | next entry |
| `p`            | previous entry |
| `N`            | next unread |
| `m`            | read, unread |
| `s`            | star, unstar |
| `o`            | in the browser |
| `y`            | copy the link |
| `v`            | play the video |
| `o<n>`         | open link n |
| `e`            | extract again |
| `<`            | narrower |
| `>`            | wider |
| `esc`          | close the article |

## feeds

_everywhere_

| key | what it does |
|---|---|
| `a`            | add a feed |
| `R/F5`         | refresh everything |
| `alt+i`        | import feeds |

## appearance

_everywhere_

| key | what it does |
|---|---|
| `t`            | next theme |
| `T`            | previous theme |
| `,`            | settings |

## application

_everywhere_

| key | what it does |
|---|---|
| `?/F1`         | this list |
| `ctrl+l`       | redraw the screen |
| `q/ctrl+c`     | quit |

## The mouse

| where | gesture | what it does |
|---|---|---|
| lists    | click                 | move the cursor |
| lists    | double-click          | open it, read it |
| entries  | right-click           | star it |
| lists    | click a crumb         | jump there |
| reader   | click a link          | open it |
| lists    | wheel                 | scroll three rows |
| modules  | click a fold          | open the module |
| modules  | click a word          | what it says |
| modules  | drag the bar          | scroll it |
| status   | click the count       | to the sources |
