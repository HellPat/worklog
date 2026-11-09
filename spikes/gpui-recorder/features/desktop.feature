Feature: Annotate a passage in the launched native fixture
  Scenario Outline: Save a note through the macOS desktop and reopen it - <case>
    Given a fresh launched fixture for "<case>"
    When I start and stop the recording simulation
    And I open and close Settings
    And I drag a range on the "<region>"
    And I enter and save an annotation
    Then the note is displayed and stored in SQLite
    And the note remains displayed after reopening the app

    Examples:
      | case    | region                    |
      | chart-1 | Voice activity chart      |
      | chart-2 | Voice activity chart      |
      | chart-3 | Voice activity chart      |
      | track-1 | Transcript range selector |

  Scenario: Refine exact message boundaries on a separate page
    Given a fresh launched fixture for "chart-refine"
    When I drag a range on the "Voice activity chart"
    And I refine the span to one exact message
    And I enter and save an annotation
    Then the note is displayed and stored in SQLite
    And the note remains displayed after reopening the app

  Scenario: Footer starts an unlabeled span and returns from settings
    Given a fresh launched fixture for "footer-start"
    When I start and stop the recording simulation
    And I open and close Settings
    And I start and end a span without details
    Then the unlabeled span is stored

  Scenario: Comment on a transcript message and reopen
    Given a fresh launched fixture for "comment"
    When I comment on the first message
    Then the comment survives reopening

  Scenario: Real audio transcript can be labeled without changing its text
    Given a fresh launched fixture for "audio"
    When I label the imported audio transcript
    And I enter and save an annotation
    Then the audio transcript and span are independently stored and linked
    And the note remains displayed after reopening the app

  Scenario: Named running span preserves optional details
    Given a fresh launched fixture for "named-span"
    When I start and end a named span
    Then the running span details are stored

  Scenario: Refinement keeps its timeline and draft through Settings
    Given a fresh launched fixture for "refine-navigation"
    When I label the imported audio transcript
    And I keep my draft through Settings and adjust the timeline
    And I enter and save an annotation
    Then the note is displayed and stored in SQLite
    And the note remains displayed after reopening the app
